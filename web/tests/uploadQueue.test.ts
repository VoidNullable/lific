import { expect, test } from "bun:test";
import { createConcurrencyQueue } from "../src/lib/attachments/queue";

/** A task whose promise resolves only when the test says so, and that records
 *  when it started. */
function deferred() {
  let resolve!: (v: string) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<string>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test("runs at most `limit` tasks at once", async () => {
  const queue = createConcurrencyQueue(3);
  const gates = Array.from({ length: 6 }, () => deferred());
  const started: number[] = [];

  const results = gates.map((gate, i) =>
    queue.add(() => {
      started.push(i);
      return gate.promise;
    }),
  );

  // Only the first three are admitted; the rest wait for a free slot.
  expect(started).toEqual([0, 1, 2]);
  expect(queue.active).toBe(3);
  expect(queue.waiting).toBe(3);

  gates[1].resolve("b");
  await results[1];
  expect(started).toEqual([0, 1, 2, 3]);
  expect(queue.active).toBe(3);

  for (const gate of gates) gate.resolve("done");
  await Promise.all(results);
  expect(started).toEqual([0, 1, 2, 3, 4, 5]);
  expect(queue.active).toBe(0);
  expect(queue.waiting).toBe(0);
});

test("a rejected task frees its slot and does not stall the queue", async () => {
  const queue = createConcurrencyQueue(2);
  const first = deferred();
  const second = deferred();
  let thirdStarted = false;

  const a = queue.add(() => first.promise);
  const b = queue.add(() => second.promise);
  const c = queue.add(async () => {
    thirdStarted = true;
    return "c";
  });

  expect(thirdStarted).toBe(false);
  first.reject(new Error("boom"));
  await expect(a).rejects.toThrow("boom");
  await c;
  expect(thirdStarted).toBe(true);

  second.resolve("b");
  await b;
  expect(queue.active).toBe(0);
});

test("a task that throws synchronously rejects rather than wedging a slot", async () => {
  const queue = createConcurrencyQueue(1);
  const boom = queue.add(() => {
    throw new Error("sync boom");
  });
  await expect(boom).rejects.toThrow("sync boom");

  const after = await queue.add(async () => "ok");
  expect(after).toBe("ok");
  expect(queue.active).toBe(0);
});

test("resolves with each task's own value, in completion order", async () => {
  const queue = createConcurrencyQueue(3);
  const values = await Promise.all([
    queue.add(async () => 1),
    queue.add(async () => 2),
    queue.add(async () => 3),
    queue.add(async () => 4),
  ]);
  expect(values).toEqual([1, 2, 3, 4]);
});

test("a limit below one is clamped to serial execution", () => {
  expect(createConcurrencyQueue(0).limit).toBe(1);
  expect(createConcurrencyQueue(-5).limit).toBe(1);
  expect(createConcurrencyQueue(3.7).limit).toBe(3);
});
