import { describe, expect, test } from "bun:test";
import { cancelBeforeFilenameResolution } from "./download-cancellation";

describe("cancelBeforeFilenameResolution", () => {
  test("starts cancellation before releasing Chrome's filename pipeline", async () => {
    const events: string[] = [];
    let settleCancellation!: () => void;
    const cancellation = new Promise<void>((resolve) => {
      settleCancellation = resolve;
    });

    const pending = cancelBeforeFilenameResolution(
      7,
      () => {
        events.push("cancel");
        return cancellation;
      },
      async () => {
        events.push("erase");
        return [];
      },
      () => events.push("suggest"),
    );

    expect(events).toEqual(["cancel", "suggest"]);
    settleCancellation();
    await expect(pending).resolves.toBe(true);
    expect(events).toEqual(["cancel", "suggest", "erase"]);
  });

  test("releases the filename pipeline but does not claim a failed cancellation", async () => {
    const events: string[] = [];

    await expect(
      cancelBeforeFilenameResolution(
        8,
        async () => {
          events.push("cancel");
          throw new Error("download disappeared");
        },
        async () => {
          events.push("erase");
          return [];
        },
        () => events.push("suggest"),
      ),
    ).resolves.toBe(false);

    expect(events).toEqual(["cancel", "suggest"]);
  });

  test("reports a successful cancellation without waiting for history erase", async () => {
    const events: string[] = [];

    await expect(
      cancelBeforeFilenameResolution(
        9,
        async () => {
          events.push("cancel");
        },
        () => {
          events.push("erase");
          return Promise.withResolvers<never>().promise;
        },
        () => events.push("suggest"),
      ),
    ).resolves.toBe(true);

    expect(events).toEqual(["cancel", "suggest", "erase"]);
  });
});
