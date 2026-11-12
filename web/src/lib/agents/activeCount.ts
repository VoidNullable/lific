// LIF-543 / LIF-544: how many agents are active right now, for the sidebar
// badge. Status is absolute (last action under 10 minutes), so the smallest
// window is enough and keeps the read cheap.

import { getAgentActivity } from "../api";

/** Active agent count, or 0 when the request fails (the badge just hides). */
export async function loadActiveAgentCount(): Promise<number> {
  const res = await getAgentActivity("1h");
  return res.ok ? res.data.active_count : 0;
}
