import { loadFont as loadSpaceGrotesk } from "@remotion/google-fonts/SpaceGrotesk";
import { loadFont as loadDMSans } from "@remotion/google-fonts/DMSans";
import { loadFont as loadFiraCode } from "@remotion/google-fonts/FiraCode";

// Mirrors the app's font stack: Space Grotesk (display), DM Sans (body),
// Fira Code standing in for the mono stack (Cascadia isn't on Google Fonts).
const grotesk = loadSpaceGrotesk("normal", {
  weights: ["500", "600", "700"],
  subsets: ["latin"],
});
const dmSans = loadDMSans("normal", {
  weights: ["400", "500", "600"],
  subsets: ["latin"],
});
const fira = loadFiraCode("normal", {
  weights: ["400", "500", "600"],
  subsets: ["latin"],
});

export const DISPLAY = grotesk.fontFamily;
export const BODY = dmSans.fontFamily;
export const MONO = fira.fontFamily;
