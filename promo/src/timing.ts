/**
 * Beat sheet in frames (30 fps). TransitionSeries overlaps scenes by the
 * transition duration, so total = sum(scenes) - sum(transitions).
 *
 * Research-derived structure (see Lific page LIF-DOC-17):
 *  cold open pain -> agitate (Jira / Linear / FOSS group) -> reveal ->
 *  terminal demo -> UI demo -> agent/MCP demo -> proof -> single CTA.
 */
export const TRANSITION = 12;

export const SCENES = {
  hook: 100,
  jira: 78,
  linear: 85,
  foss: 250,
  reveal: 120,
  terminal: 270,
  ui: 240,
  agent: 215,
  teams: 270,
  cta: 265,
} as const;

const durations = Object.values(SCENES);
export const TOTAL_FRAMES =
  durations.reduce((a, b) => a + b, 0) - (durations.length - 1) * TRANSITION;
