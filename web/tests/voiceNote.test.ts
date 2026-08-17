import { describe, expect, test } from "bun:test";
import {
  VOICE_MIME_CANDIDATES,
  extensionForMime,
  formatElapsed,
  meterLevel,
  pickAudioMime,
  voiceNoteFilename,
} from "../src/lib/attachments/voiceNote";

describe("voiceNoteFilename", () => {
  test("stamps local date and time to the minute", () => {
    const at = new Date(2026, 7, 17, 14, 5);
    expect(voiceNoteFilename(at)).toBe("voice-note-20260817-1405.webm");
  });

  test("zero-pads every field", () => {
    const at = new Date(2026, 0, 3, 9, 7);
    expect(voiceNoteFilename(at)).toBe("voice-note-20260103-0907.webm");
  });

  test("sorts lexicographically in chronological order", () => {
    const earlier = voiceNoteFilename(new Date(2026, 8, 9, 8, 0));
    const later = voiceNoteFilename(new Date(2026, 8, 10, 8, 0));
    expect([later, earlier].sort()).toEqual([earlier, later]);
  });

  test("follows the container the platform actually recorded", () => {
    expect(voiceNoteFilename(new Date(2026, 7, 17, 14, 5), "audio/mp4")).toBe(
      "voice-note-20260817-1405.m4a",
    );
    expect(voiceNoteFilename(new Date(2026, 7, 17, 14, 5), "audio/ogg;codecs=opus")).toBe(
      "voice-note-20260817-1405.ogg",
    );
  });
});

describe("extensionForMime", () => {
  test("ignores codec parameters", () => {
    expect(extensionForMime("audio/webm;codecs=opus")).toBe("webm");
  });

  test("falls back to webm for anything unrecognised", () => {
    expect(extensionForMime("audio/flac")).toBe("webm");
  });
});

describe("pickAudioMime", () => {
  test("prefers opus in webm when it is available", () => {
    expect(pickAudioMime(() => true)).toBe("audio/webm;codecs=opus");
  });

  test("falls through to the first supported container", () => {
    expect(pickAudioMime((mime) => mime === "audio/mp4")).toBe("audio/mp4");
  });

  test("returns undefined when nothing is supported", () => {
    expect(pickAudioMime(() => false)).toBeUndefined();
  });

  test("treats a throwing isTypeSupported as unsupported", () => {
    expect(
      pickAudioMime((mime) => {
        if (mime !== "audio/ogg;codecs=opus") throw new Error("nope");
        return true;
      }),
    ).toBe("audio/ogg;codecs=opus");
  });

  test("every candidate is a concrete audio type", () => {
    for (const mime of VOICE_MIME_CANDIDATES) expect(mime.startsWith("audio/")).toBe(true);
  });
});

describe("formatElapsed", () => {
  test("renders m:ss under an hour", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(7_400)).toBe("0:07");
    expect(formatElapsed(65_000)).toBe("1:05");
    expect(formatElapsed(600_000)).toBe("10:00");
  });

  test("adds an hours field past an hour", () => {
    expect(formatElapsed(3_725_000)).toBe("1:02:05");
  });

  test("clamps a negative elapsed time to zero", () => {
    expect(formatElapsed(-500)).toBe("0:00");
  });
});

describe("meterLevel", () => {
  test("reads silence as zero", () => {
    expect(meterLevel(new Uint8Array(64).fill(128))).toBe(0);
  });

  test("saturates at one for a full-scale signal", () => {
    expect(meterLevel(new Uint8Array(64).fill(255))).toBe(1);
  });

  test("scales quiet speech into a visible range", () => {
    // ±13/128 is roughly conversational RMS; the meter should read it as
    // clearly alive rather than a dead bar.
    const wave = Array.from({ length: 64 }, (_, i) => 128 + (i % 2 === 0 ? 13 : -13));
    const level = meterLevel(wave);
    expect(level).toBeGreaterThan(0.35);
    expect(level).toBeLessThan(0.6);
  });

  test("handles an empty buffer", () => {
    expect(meterLevel(new Uint8Array(0))).toBe(0);
  });
});
