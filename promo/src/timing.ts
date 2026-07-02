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
  coldOpen: 100,
  jira: 105,
  linear: 115,
  foss: 150,
  reveal: 160,
  terminal: 360,
  ui: 370,
  agent: 250,
  proof: 160,
  cta: 130,
} as const;

const durations = Object.values(SCENES);
export const TOTAL_FRAMES =
  durations.reduce((a, b) => a + b, 0) - (durations.length - 1) * TRANSITION;
