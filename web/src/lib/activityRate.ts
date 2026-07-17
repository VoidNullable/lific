export type ActivityCounts = {
  perSecond: number;
  perMinute: number;
  perHour: number;
  perDay: number;
};

export type ActivityCountsReader = (now: number) => ActivityCounts;

export type ActivityRate = {
  value: number;
  unit: "updates/s" | "updates/min" | "updates/hr" | "updates/day";
};

const EVENT_COMPACTION_THRESHOLD = 4_096;

export function createActivityRateCounter() {
  const eventTimes: number[] = [];
  let eventHead = 0;
  const minuteBuckets: { minute: number; count: number }[] = [];

  function prune(now: number) {
    const minuteAgo = now - 60_000;
    while (eventHead < eventTimes.length && eventTimes[eventHead] < minuteAgo) {
      eventHead += 1;
    }
    if (eventHead > EVENT_COMPACTION_THRESHOLD && eventHead * 2 > eventTimes.length) {
      eventTimes.splice(0, eventHead);
      eventHead = 0;
    }

    const dayAgo = now - 86_400_000;
    while (minuteBuckets.length > 0 && (minuteBuckets[0].minute + 1) * 60_000 <= dayAgo) {
      minuteBuckets.shift();
    }
  }

  function record(now: number) {
    prune(now);
    eventTimes.push(now);
    const minute = Math.floor(now / 60_000);
    const bucket = minuteBuckets.at(-1);
    if (bucket?.minute === minute) {
      bucket.count += 1;
    } else {
      minuteBuckets.push({ minute, count: 1 });
    }
  }

  function counts(now: number): ActivityCounts {
    prune(now);
    let perSecond = 0;
    for (let i = eventTimes.length - 1; i >= eventHead; i -= 1) {
      if (eventTimes[i] < now - 1_000) break;
      perSecond += 1;
    }
    let perHour = 0;
    const hourAgo = now - 3_600_000;
    for (let i = minuteBuckets.length - 1; i >= 0; i -= 1) {
      const bucket = minuteBuckets[i];
      if ((bucket.minute + 1) * 60_000 <= hourAgo) break;
      perHour += bucket.count;
    }
    return {
      perSecond,
      perMinute: eventTimes.length - eventHead,
      perHour,
      perDay: minuteBuckets.reduce((total, bucket) => total + bucket.count, 0),
    };
  }

  function reset() {
    eventTimes.length = 0;
    eventHead = 0;
    minuteBuckets.length = 0;
  }

  return { record, counts, reset };
}

export function selectActivityRate(counts: ActivityCounts): ActivityRate {
  // Two or more events select the shortest meaningful window; otherwise the
  // trailing day remains the conservative fallback.
  if (counts.perSecond >= 2) return { value: counts.perSecond, unit: "updates/s" };
  if (counts.perMinute >= 2) return { value: counts.perMinute, unit: "updates/min" };
  if (counts.perHour >= 2) return { value: counts.perHour, unit: "updates/hr" };
  return { value: counts.perDay, unit: "updates/day" };
}
