<script lang="ts">
  // LIF-418: record a voice note straight into a composer.
  //
  // The whole point is that it costs one tap. Press, talk, press stop, listen
  // back if you want, attach. It uploads through the ordinary attachment path
  // as a normal file, so it inherits linking, the pending chip, the retry
  // affordance and whatever the server's size and MIME rules happen to be.
  //
  // No special-casing of server responses lives here. If the backend rejects
  // audio/webm the upload chip shows the server's own sentence, which is the
  // correct behaviour whether the allowlist is missing the type, the file is
  // too big, or the disk is full.
  //
  // MediaRecorder is feature-detected after mount (so server-side rendering
  // never touches it) and the button simply does not exist when unsupported.

  import { Mic, Square, Trash2, Check } from "lucide-svelte";
  import {
    formatElapsed,
    meterLevel,
    pickAudioMime,
    voiceNoteFilename,
  } from "./voiceNote";

  let {
    onFile,
    disabled = false,
    /** False when a parent (the mobile attach menu) supplies its own trigger
     *  and drives this component through `start()`. */
    showButton = true,
  }: {
    onFile: (file: File) => void;
    disabled?: boolean;
    showButton?: boolean;
  } = $props();

  // Ten minutes. Long enough that nobody hits it describing a bug, short
  // enough that a forgotten open tab cannot record the rest of the afternoon.
  const MAX_MS = 10 * 60 * 1000;

  type Phase = "idle" | "requesting" | "recording" | "preview";

  let phase = $state<Phase>("idle");
  let supported = $state(false);
  let elapsed = $state(0);
  let level = $state(0);
  let error = $state<string | null>(null);
  let previewUrl = $state<string | null>(null);

  let stream: MediaStream | null = null;
  let recorder: MediaRecorder | null = null;
  let audioCtx: AudioContext | null = null;
  let analyser: AnalyserNode | null = null;
  let samples: Uint8Array<ArrayBuffer> | null = null;
  let chunks: Blob[] = [];
  let recorded: Blob | null = null;
  let recordedMime = "audio/webm";
  let cancelled = false;
  let raf = 0;
  let startedAt = 0;

  function detect(): boolean {
    if (typeof window === "undefined") return false;
    if (typeof MediaRecorder === "undefined") return false;
    if (!navigator.mediaDevices?.getUserMedia) return false;
    return pickAudioMime((mime) => MediaRecorder.isTypeSupported(mime)) !== undefined;
  }

  $effect(() => {
    supported = detect();
    return () => teardown();
  });

  export function isSupported(): boolean {
    return supported;
  }

  function teardown() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    stream?.getTracks().forEach((track) => track.stop());
    stream = null;
    analyser = null;
    samples = null;
    void audioCtx?.close().catch(() => {});
    audioCtx = null;
    recorder = null;
  }

  function releasePreview() {
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    previewUrl = null;
    recorded = null;
  }

  function tick() {
    elapsed = performance.now() - startedAt;
    if (analyser && samples) {
      analyser.getByteTimeDomainData(samples);
      // Ease downward so the meter reads as a level, not a strobe.
      level = Math.max(meterLevel(samples), level * 0.82);
    }
    if (elapsed >= MAX_MS) {
      stop();
      return;
    }
    raf = requestAnimationFrame(tick);
  }

  export async function start() {
    if (!supported || phase !== "idle" || disabled) return;
    error = null;
    releasePreview();
    phase = "requesting";
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch {
      phase = "idle";
      error = "Microphone unavailable. Check the browser's permission for this site.";
      return;
    }

    const mime = pickAudioMime((candidate) => MediaRecorder.isTypeSupported(candidate));
    recordedMime = (mime ?? "audio/webm").split(";")[0];
    try {
      recorder = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    } catch {
      teardown();
      phase = "idle";
      error = "This browser could not start an audio recorder.";
      return;
    }

    chunks = [];
    cancelled = false;
    recorder.ondataavailable = (e) => {
      if (e.data.size > 0) chunks.push(e.data);
    };
    recorder.onstop = () => {
      const collected = new Blob(chunks, { type: recordedMime });
      chunks = [];
      teardown();
      if (cancelled || collected.size === 0) {
        cancelled = false;
        phase = "idle";
        return;
      }
      recorded = collected;
      previewUrl = URL.createObjectURL(collected);
      phase = "preview";
    };

    try {
      // A timeslice keeps chunks flowing so a crash mid-recording does not
      // lose everything the recorder was buffering.
      recorder.start(1000);
    } catch {
      teardown();
      phase = "idle";
      error = "This browser could not start an audio recorder.";
      return;
    }

    try {
      audioCtx = new AudioContext();
      analyser = audioCtx.createAnalyser();
      analyser.fftSize = 512;
      audioCtx.createMediaStreamSource(stream).connect(analyser);
      samples = new Uint8Array(new ArrayBuffer(analyser.fftSize));
    } catch {
      // The level meter is decoration. Losing it must not lose the recording.
      analyser = null;
      samples = null;
    }

    startedAt = performance.now();
    elapsed = 0;
    level = 0;
    phase = "recording";
    raf = requestAnimationFrame(tick);
  }

  function stop() {
    if (phase !== "recording" || !recorder) return;
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    recorder.stop();
  }

  function cancel() {
    if (phase === "recording" && recorder) {
      cancelled = true;
      recorder.stop();
      return;
    }
    releasePreview();
    teardown();
    phase = "idle";
    error = null;
  }

  function attach() {
    if (!recorded) return;
    const file = new File([recorded], voiceNoteFilename(new Date(), recordedMime), {
      type: recordedMime,
      lastModified: Date.now(),
    });
    releasePreview();
    phase = "idle";
    elapsed = 0;
    level = 0;
    onFile(file);
  }

  function onPanelKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      cancel();
    }
  }
</script>

{#if supported}
  <span class="vn">
    {#if showButton}
      <button
        type="button"
        class="vn__button"
        class:is-live={phase === "recording"}
        title="Record a voice note"
        aria-label="Record a voice note"
        aria-pressed={phase === "recording"}
        disabled={disabled || phase === "requesting"}
        onclick={() => (phase === "recording" ? stop() : phase === "idle" ? start() : cancel())}
      >
        <Mic size={13} />
      </button>
    {/if}

    {#if phase !== "idle" || error}
      <!-- Escape cancels while focus is anywhere in the panel; the panel itself
           is never focused, so this is a convenience listener, not the only
           route out (every action has a real button). -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div class="vn__panel" role="group" aria-label="Voice note" onkeydown={onPanelKeydown}>
        {#if error}
          <p class="vn__error">{error}</p>
          <button type="button" class="vn__ghost" onclick={() => (error = null)}>Dismiss</button>
        {:else if phase === "requesting"}
          <span class="vn__status">Waiting for the microphone…</span>
        {:else if phase === "recording"}
          <span class="vn__dot" aria-hidden="true"></span>
          <span class="vn__time">{formatElapsed(elapsed)}</span>
          <span class="vn__meter" aria-hidden="true">
            <span class="vn__meter-fill" style:transform={`scaleX(${level})`}></span>
          </span>
          <button type="button" class="vn__ghost" onclick={cancel}>Cancel</button>
          <button type="button" class="vn__primary" onclick={stop}>
            <Square size={11} /> Stop
          </button>
        {:else if phase === "preview" && previewUrl}
          <audio class="vn__audio" src={previewUrl} controls preload="metadata"></audio>
          <button type="button" class="vn__ghost" title="Discard" aria-label="Discard" onclick={cancel}>
            <Trash2 size={13} />
          </button>
          <button type="button" class="vn__primary" onclick={attach}>
            <Check size={11} /> Attach
          </button>
        {/if}
      </div>
    {/if}
  </span>
{/if}

<style>
  .vn {
    position: relative;
    display: inline-flex;
  }

  .vn__button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      border-color 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .vn__button:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  .vn__button.is-live {
    border-color: var(--error);
    color: var(--error);
  }
  .vn__button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Anchored above the toolbar rather than pushed inline: the composer must
     not reflow (and shove the send button around) the moment you start
     talking. */
  .vn__panel {
    position: absolute;
    left: 0;
    bottom: calc(100% + 0.5rem);
    z-index: 40;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4375rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.16);
    white-space: nowrap;
  }

  .vn__dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 999px;
    background: var(--error);
    animation: vn-pulse 1.2s ease-in-out infinite;
  }
  .vn__status,
  .vn__time {
    font-size: var(--text-caption);
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .vn__error {
    margin: 0;
    max-width: 16rem;
    white-space: normal;
    font-size: var(--text-caption);
    color: var(--error);
  }

  .vn__meter {
    display: block;
    width: 3.5rem;
    height: 0.375rem;
    border-radius: 999px;
    background: var(--bg-subtle);
    overflow: hidden;
  }
  .vn__meter-fill {
    display: block;
    width: 100%;
    height: 100%;
    background: var(--accent);
    transform-origin: left center;
    transform: scaleX(0);
  }

  .vn__audio {
    height: 2rem;
    max-width: 13rem;
  }

  .vn__ghost,
  .vn__primary {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    font-size: var(--text-caption);
    font-weight: 500;
    cursor: pointer;
  }
  .vn__ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
  }
  .vn__ghost:hover {
    color: var(--text);
    border-color: var(--text-faint);
  }
  .vn__primary {
    border: 0;
    background: var(--accent);
    color: var(--accent-text);
  }
  .vn__primary:hover {
    background: var(--accent-hover);
  }

  @keyframes vn-pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .vn__dot {
      animation: none;
    }
    .vn__button {
      transition: none;
    }
  }
</style>
