export type ActivityCounts = {
  perSecond: number;
  perMinute: number;
  perHour: number;
  perDay: number;
};

export type ActivityCountsReader = (now: number) => ActivityCounts;

type ActivityBucket = {
  at: number;
  count: number;
};

export type ActivityBaseline = {
  dayCount: number;
};

export function parseActivityBaseline(dayCount: unknown): ActivityBaseline | null {
  return typeof dayCount === "number" && Number.isSafeInteger(dayCount) && dayCount >= 0
    ? { dayCount }
    : null;
}

export type ActivityRate = {
  value: number;
  unit: "updates/s" | "updates/min" | "updates/hr" | "updates/day";
};

const DAY_MS = 86_400_000;
const ACTIVITY_REALTIME_EVENT_TYPES = new Set([
  "project.created",
  "project.updated",
  "project.deleted",
  "issue.created",
  "issue.updated",
  "issue.deleted",
  "issue.linked",
  "issue.unlinked",
]);

export function isActivityRealtimeEvent(type: string): boolean {
  return ACTIVITY_REALTIME_EVENT_TYPES.has(type);
}

export function createActivityRateCounter() {
  const secondBuckets: ActivityBucket[] = [];
  const minuteBuckets: ActivityBucket[] = [];
  let baselineDayCount = 0;
  let baselineAt: number | null = null;

  function prune(now: number) {
    const minuteAgo = now - 60_000;
    while (secondBuckets.length > 0 && secondBuckets[0].at < minuteAgo) {
      secondBuckets.shift();
    }
    const dayAgo = now - DAY_MS;
    while (minuteBuckets.length > 0 && minuteBuckets[0].at < dayAgo) {
      minuteBuckets.shift();
    }
  }

  function addBucket(buckets: ActivityBucket[], at: number, count = 1) {
    const bucket = buckets.at(-1);
    if (bucket?.at === at) {
      bucket.count += count;
    } else {
      buckets.push({ at, count });
    }
  }

  function record(now: number) {
    prune(now);
    addBucket(secondBuckets, Math.floor(now / 1_000) * 1_000);
    addBucket(minuteBuckets, Math.floor(now / 60_000) * 60_000);
  }

  function seed(baseline: ActivityBaseline, now: number) {
    secondBuckets.length = 0;
    minuteBuckets.length = 0;
    baselineDayCount = baseline.dayCount;
    baselineAt = now;
  }

  function sumSince(buckets: ActivityBucket[], now: number, window: number) {
    const cutoff = now - window;
    return buckets.reduce(
      (total, bucket) => total + (bucket.at >= cutoff ? bucket.count : 0),
      0,
    );
  }

  function counts(now: number): ActivityCounts {
    prune(now);
    return {
      perSecond: sumSince(secondBuckets, now, 1_000),
      perMinute: sumSince(secondBuckets, now, 60_000),
      perHour: sumSince(minuteBuckets, now, 3_600_000),
      perDay:
        (baselineAt !== null && now - baselineAt < DAY_MS ? baselineDayCount : 0) +
        sumSince(minuteBuckets, now, DAY_MS),
    };
  }

  function reset() {
    secondBuckets.length = 0;
    minuteBuckets.length = 0;
    baselineDayCount = 0;
    baselineAt = null;
  }

  return { record, seed, counts, reset };
}

export function selectActivityRate(counts: ActivityCounts): ActivityRate {
  // Two or more events select the shortest meaningful window; otherwise the
  // trailing day remains the conservative fallback.
  if (counts.perSecond >= 2) return { value: counts.perSecond, unit: "updates/s" };
  if (counts.perMinute >= 2) return { value: counts.perMinute, unit: "updates/min" };
  if (counts.perHour >= 2) return { value: counts.perHour, unit: "updates/hr" };
  return { value: counts.perDay, unit: "updates/day" };
}
