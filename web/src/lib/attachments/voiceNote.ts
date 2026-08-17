// LIF-418: pure helpers for the voice-note recorder.
//
// VoiceNote.svelte owns getUserMedia, MediaRecorder and the AnalyserNode; the
// naming, formatting and signal math live here so they can be tested without a
// microphone (web/tests/voiceNote.test.ts).

/**
 * Container/codec preferences, best first.
 *
 * Opus in WebM is the target: small, open, and what Chrome and Firefox record
 * natively. Safari only offers MP4/AAC, so it is kept as a last resort rather
 * than failing the feature outright. The exact string matters because
 * MediaRecorder rejects anything `isTypeSupported` did not bless.
 */
export const VOICE_MIME_CANDIDATES = [
  "audio/webm;codecs=opus",
  "audio/webm",
  "audio/ogg;codecs=opus",
  "audio/mp4",
] as const;

/** First candidate the platform can actually record, or undefined when none
 *  are supported (caller should hide the affordance). */
export function pickAudioMime(
  isSupported: (mime: string) => boolean,
  candidates: readonly string[] = VOICE_MIME_CANDIDATES,
): string | undefined {
  return candidates.find((mime) => {
    try {
      return isSupported(mime);
    } catch {
      return false;
    }
  });
}

/** File extension for a recorder MIME, ignoring any `;codecs=` suffix. */
export function extensionForMime(mime: string): string {
  const base = mime.split(";")[0].trim().toLowerCase();
  if (base === "audio/ogg") return "ogg";
  if (base === "audio/mp4") return "m4a";
  if (base === "audio/mpeg") return "mp3";
  return "webm";
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/**
 * `voice-note-YYYYMMDD-HHMM.webm`, in local time.
 *
 * Sortable, filesystem-safe, and readable at a glance in an attachment list.
 * Local time rather than UTC because the person listening later is reasoning
 * about when they recorded it, not about an absolute instant. Two recordings
 * inside the same minute collide by name; the server assigns ids, so that is
 * cosmetic.
 */
export function voiceNoteFilename(now: Date, mime = "audio/webm;codecs=opus"): string {
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}`;
  return `voice-note-${stamp}.${extensionForMime(mime)}`;
}

/** Elapsed time as `m:ss`, or `h:mm:ss` past an hour. Clamped at zero so a
 *  clock skew mid-recording cannot render a negative timer. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  if (hours > 0) return `${hours}:${pad(minutes)}:${pad(seconds)}`;
  return `${minutes}:${pad(seconds)}`;
}

/**
 * Normalised 0..1 loudness from an AnalyserNode time-domain buffer.
 *
 * Byte time-domain samples centre on 128, so RMS is taken around that. The
 * result is scaled by 4 and clipped: raw speech RMS sits near 0.05-0.15, which
 * would render as a permanently dead meter, and the meter's job is "the mic is
 * hearing you", not calibrated metering.
 */
export function meterLevel(samples: Uint8Array | number[]): number {
  if (samples.length === 0) return 0;
  let sum = 0;
  for (let i = 0; i < samples.length; i += 1) {
    const deviation = (samples[i] - 128) / 128;
    sum += deviation * deviation;
  }
  const rms = Math.sqrt(sum / samples.length);
  return Math.min(1, rms * 4);
}
