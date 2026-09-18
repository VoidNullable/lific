// LIF-443: one websocket per browser, not one per tab.
//
// Ten open Lific tabs used to mean ten websockets, ten resume replays and
// ten copies of every event the server pushed. This module elects a single
// leader tab per origin, gives only that tab a socket, and fans the traffic
// out to the rest over a BroadcastChannel.
//
// ── Election ────────────────────────────────────────────────
//
// The Web Locks API does the whole job. Every connected tab asks for the
// same exclusive lock, `lific-sync`, and the callback returns a promise the
// module never resolves while it is connected. The browser grants the lock
// to exactly one tab and queues the others; the holder keeps it for the
// lifetime of the tab. When that tab closes, crashes, or logs out, the lock
// is released and the browser hands it to the next waiter, which opens a
// socket and takes over. There is no heartbeat, no timeout, no tie-break:
// the lock manager is the source of truth and it is crash-safe by
// construction.
//
// The lock name is instance-wide, not per-project, because the socket is
// instance-scoped: one connection carries every project's events.
//
// ── Duplicate tolerance (why the fan-out is safe) ───────────
//
// Followers can and do send outbound frames, most importantly the `resume`
// frame their own read model needs after a promotion. Those frames go over
// the channel and the leader forwards them, so the server may replay the
// same range more than once and every tab sees the replay, not just the tab
// that asked. That is harmless: realtime envelopes never advance a read
// model's cursor (see readModel.svelte.ts), so a replayed event whose `seq`
// is at or below the cursor is dropped, and anything above it collapses
// into the same debounced `/changes?since=cursor` pull the model would have
// issued anyway. Duplicates cost a pull that returns nothing.
//
// ── Fallback ────────────────────────────────────────────────
//
// Without `navigator.locks` (or `BroadcastChannel`) there is no election
// and no channel: every tab is its own leader with its own socket, exactly
// the behaviour that shipped before this module existed. `shared` is false,
// `leader` is true from construction, `post()` is a no-op, and every code
// path below collapses to "open a socket, hand its events to the caller".

import type { RealtimeEvent } from "../autoRefresh.svelte";

const LOCK_NAME = "lific-sync";
const CHANNEL_NAME = "lific-sync-events";

/** Wire format on the BroadcastChannel. `kind` is deliberately not `type`,
 *  which realtime envelopes already use for their own vocabulary. */
type ChannelMessage =
  /** Leader → followers: a message the socket just delivered, parsed. */
  | { kind: "event"; event: RealtimeEvent }
  /** Leader → followers: the shared socket is up (also sent on promotion). */
  | { kind: "open" }
  /** Leader → followers: the shared socket went away. */
  | { kind: "close" }
  /** Follower → leader: send this frame on the shared socket. */
  | { kind: "outbound"; frame: unknown }
  /** Follower → leader: "are you up?" A live leader answers with `open`,
   *  which is how a tab opened mid-session learns the connection exists. */
  | { kind: "hello" };

export interface SyncClientOptions {
  /** Resolved lazily so the caller keeps ownership of the URL scheme. */
  url: () => string;
  /** One realtime envelope, delivered in every tab regardless of role. */
  onEvent: (event: RealtimeEvent) => void;
  /** The shared connection came up. Fires in every tab, including after a
   *  promotion, which is the caller's cue to resume + resync. */
  onOpen: () => void;
  /** The shared connection went away. Fires in every tab. Not fired by
   *  `disconnect()`, whose caller is already doing its own teardown. */
  onClose: () => void;
  /** Leader-only: our socket closed and nothing will reopen it on its own.
   *  `opened` is whether it ever reached OPEN, which is what separates "the
   *  server dropped us" from "the connection was refused". The caller owns
   *  backoff and reauth policy and calls `connect()` again when ready. */
  onLeaderDisconnect: (opened: boolean) => void;
}

export interface SyncClient {
  /** Idempotent. Starts the election (or reopens the leader's socket after
   *  a disconnect). Safe to call on every route change. */
  connect(): void;
  /** Drops the socket, releases the lock so another tab can lead, and stops
   *  listening. Tells followers the connection is gone. */
  disconnect(): void;
  /** Send an outbound frame. The leader writes it to the socket; a follower
   *  asks the leader to. Returns false when there is nowhere to send it. */
  send(frame: unknown): boolean;
  /** True when this tab owns the socket (leader, or the no-Locks fallback).
   *  Callers use it to keep per-connection chores off the followers. */
  ownsSocket(): boolean;
  /** True while a shared connection exists or is being established, from
   *  this tab's point of view. Followers report true because they have
   *  nothing of their own to reconnect. */
  hasLiveConnection(): boolean;
}

/** Whether this browser can share one socket across tabs. */
export function supportsSharedSync(): boolean {
  return (
    typeof navigator !== "undefined" &&
    typeof navigator.locks?.request === "function" &&
    typeof BroadcastChannel === "function"
  );
}

export function createSyncClient(options: SyncClientOptions): SyncClient {
  const shared = supportsSharedSync();

  /** The caller wants a connection. Everything below is a no-op without it. */
  let desired = false;
  /** Owns the socket. Without Web Locks every tab does, forever. */
  let leader = !shared;
  let socket: WebSocket | null = null;
  let channel: BroadcastChannel | null = null;
  /** Resolving this hands the lock to the next waiting tab. */
  let releaseLock: (() => void) | null = null;
  /** Aborts a lock request still queued behind another tab. */
  let electing: AbortController | null = null;
  /** Follower-side view of the shared connection, so a second `open` with
   *  no `close` between them is recognised as a leader that died abruptly. */
  let followerOpen = false;

  function post(message: ChannelMessage): void {
    channel?.postMessage(message);
  }

  function sendOnSocket(frame: unknown): boolean {
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(frame));
      return true;
    }
    return false;
  }

  function openSocket(): void {
    if (socket) return;
    const ws = new WebSocket(options.url());
    socket = ws;
    let opened = false;

    // Every listener re-checks `socket === ws`: a torn-down or superseded
    // socket must not drive the app, and this is the same guard the socket
    // handling used before it moved into this module.
    ws.addEventListener("open", () => {
      if (socket !== ws) return;
      opened = true;
      post({ kind: "open" });
      options.onOpen();
    });

    ws.addEventListener("message", (message) => {
      if (socket !== ws || typeof message.data !== "string") return;
      let event: RealtimeEvent;
      try {
        event = JSON.parse(message.data) as RealtimeEvent;
      } catch {
        // HTTP refresh remains source of truth.
        return;
      }
      if (typeof event?.type !== "string") return;
      post({ kind: "event", event });
      options.onEvent(event);
    });

    ws.addEventListener("close", () => {
      if (socket !== ws) return;
      socket = null;
      post({ kind: "close" });
      options.onClose();
      options.onLeaderDisconnect(opened);
    });

    ws.addEventListener("error", () => {
      ws.close();
    });
  }

  function teardownSocket(): void {
    const ws = socket;
    socket = null;
    if (!ws) return;
    // Null-first, so the close listener's guard fires and the caller does
    // not get an `onClose` for a teardown it asked for.
    if (ws.readyState === WebSocket.OPEN) post({ kind: "close" });
    ws.close(1000, "teardown");
  }

  function becomeLeader(): void {
    leader = true;
    // Promotion after an abrupt leader death: this tab still believes the
    // old connection is up. Surface the loss before the new socket comes
    // up, so the caller marks itself as needing a full resync.
    if (followerOpen) {
      followerOpen = false;
      options.onClose();
    }
    openSocket();
  }

  function elect(): void {
    if (leader || electing || !navigator.locks) return;
    const controller = new AbortController();
    electing = controller;
    navigator.locks
      .request(
        LOCK_NAME,
        { mode: "exclusive", signal: controller.signal },
        () =>
          // Held, never resolved, for as long as this tab wants a
          // connection. That is what makes the lock a leadership term.
          new Promise<void>((resolve) => {
            electing = null;
            if (!desired) {
              resolve();
              return;
            }
            releaseLock = resolve;
            becomeLeader();
          }),
      )
      .catch(() => {
        // Aborted by disconnect(), or the lock manager refused. Either way
        // this tab is not the leader; a later connect() re-enters.
        if (electing === controller) electing = null;
      });
  }

  function handleChannelMessage(message: ChannelMessage): void {
    if (!desired) return;

    if (leader) {
      if (message.kind === "outbound") {
        sendOnSocket(message.frame);
      } else if (
        message.kind === "hello" &&
        socket?.readyState === WebSocket.OPEN
      ) {
        post({ kind: "open" });
      }
      return;
    }

    switch (message.kind) {
      case "event":
        options.onEvent(message.event);
        break;
      case "open":
        // Two opens with no close between them means the previous leader
        // vanished without saying goodbye. Report the gap so the caller
        // treats the new connection as a reconnect, not a first connect.
        if (followerOpen) options.onClose();
        followerOpen = true;
        options.onOpen();
        break;
      case "close":
        if (followerOpen) {
          followerOpen = false;
          options.onClose();
        }
        break;
    }
  }

  return {
    connect(): void {
      desired = true;
      if (shared && !channel) {
        channel = new BroadcastChannel(CHANNEL_NAME);
        channel.addEventListener("message", (event: MessageEvent) => {
          handleChannelMessage(event.data as ChannelMessage);
        });
        // A tab opened mid-session has missed the leader's `open`; ask for
        // it rather than waiting for the next reconnect.
        post({ kind: "hello" });
      }
      if (leader) openSocket();
      else elect();
    },

    disconnect(): void {
      desired = false;
      followerOpen = false;
      teardownSocket();
      if (electing) {
        electing.abort();
        electing = null;
      }
      if (releaseLock) {
        releaseLock();
        releaseLock = null;
      }
      // Give up the term so another tab can lead. In the fallback there is
      // no term to give up and this tab stays its own leader.
      if (shared) leader = false;
      channel?.close();
      channel = null;
    },

    send(frame: unknown): boolean {
      if (leader) return sendOnSocket(frame);
      if (!desired || !followerOpen || !channel) return false;
      post({ kind: "outbound", frame });
      return true;
    },

    ownsSocket(): boolean {
      return leader;
    },

    hasLiveConnection(): boolean {
      if (leader) {
        return (
          socket?.readyState === WebSocket.OPEN ||
          socket?.readyState === WebSocket.CONNECTING
        );
      }
      return desired;
    },
  };
}
