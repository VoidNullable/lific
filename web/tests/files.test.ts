import { describe, expect, test } from "bun:test";
import {
  canDeleteAttachment,
  deleteConfirmMessage,
  entityChipLabel,
  entityHref,
  formatSweepCountdown,
  mimeClassLabel,
  uploaderOptions,
} from "../src/lib/files/files";
import type { LinkedEntity, ProjectAttachment } from "../src/lib/api";

function entity(partial: Partial<LinkedEntity> = {}): LinkedEntity {
  return {
    entity_type: "issue",
    entity_id: 1,
    identifier: "LIF-1",
    title: "An issue",
    page_id: null,
    ...partial,
  };
}

function attachment(partial: Partial<ProjectAttachment> = {}): ProjectAttachment {
  return {
    id: 1,
    filename: "shot.png",
    mime: "image/png",
    mime_class: "image",
    size_bytes: 10,
    uploader_id: 7,
    uploader: "blake",
    uploader_display_name: "Blake",
    created_at: "2026-08-17 12:00:00",
    entities: [],
    ...partial,
  };
}

describe("sweep countdown", () => {
  test("rounds to the coarsest useful unit", () => {
    expect(formatSweepCountdown(60 * 60 * 47)).toBe("swept in 1 day");
    expect(formatSweepCountdown(60 * 60 * 49)).toBe("swept in 2 days");
    expect(formatSweepCountdown(60 * 60 * 5)).toBe("swept in 5h");
    expect(formatSweepCountdown(90)).toBe("swept in 1 min");
  });

  test("a file past the grace window goes on the next pass", () => {
    expect(formatSweepCountdown(0)).toBe("swept on the next pass");
    expect(formatSweepCountdown(-500)).toBe("swept on the next pass");
  });
});

describe("delete confirm", () => {
  test("states how many references go with the file", () => {
    expect(deleteConfirmMessage(1)).toBe(
      "Removes the file and its 1 reference.",
    );
    expect(deleteConfirmMessage(3)).toBe(
      "Removes the file and its 3 references.",
    );
    expect(deleteConfirmMessage(0)).toBe(
      "Removes the file. It has no references.",
    );
  });
});

describe("delete permission mirror", () => {
  const base = { uploaderId: 7, viewerId: 9, isAdmin: false, canEdit: false };

  test("admins and maintainers may delete anyone's file", () => {
    expect(canDeleteAttachment({ ...base, isAdmin: true })).toBe(true);
    expect(canDeleteAttachment({ ...base, canEdit: true })).toBe(true);
  });

  test("a viewer may delete only their own upload", () => {
    expect(canDeleteAttachment(base)).toBe(false);
    expect(canDeleteAttachment({ ...base, viewerId: 7 })).toBe(true);
  });

  test("fails open while the viewer is unknown", () => {
    expect(canDeleteAttachment({ ...base, viewerId: null })).toBe(true);
  });

  test("an uploaderless file is not deletable by a plain viewer", () => {
    expect(canDeleteAttachment({ ...base, uploaderId: null })).toBe(false);
  });
});

describe("linked entity chips", () => {
  test("issues route by identifier, pages by numeric id", () => {
    expect(entityHref("LIF", entity())).toBe("/LIF/issues/LIF-1");
    expect(
      entityHref(
        "LIF",
        entity({ entity_type: "page", identifier: "LIF-DOC-2", page_id: 42 }),
      ),
    ).toBe("/LIF/pages/42");
  });

  test("a comment lands on the entity its thread lives on", () => {
    expect(entityHref("LIF", entity({ entity_type: "comment", entity_id: 5 }))).toBe(
      "/LIF/issues/LIF-1",
    );
    expect(
      entityHref(
        "LIF",
        entity({ entity_type: "comment", entity_id: 5, page_id: 42 }),
      ),
    ).toBe("/LIF/pages/42");
  });

  test("an unresolvable link has no route", () => {
    expect(entityHref("LIF", entity({ identifier: null }))).toBeNull();
  });

  test("a comment reference says so on the chip", () => {
    expect(entityChipLabel(entity())).toBe("LIF-1");
    expect(entityChipLabel(entity({ entity_type: "comment" }))).toBe(
      "LIF-1 (comment)",
    );
  });
});

describe("filter options", () => {
  test("uploader options are distinct, sorted, and skip deleted accounts", () => {
    expect(
      uploaderOptions([
        attachment({ id: 1, uploader: "zoe" }),
        attachment({ id: 2, uploader: "blake" }),
        attachment({ id: 3, uploader: "zoe" }),
        attachment({ id: 4, uploader: null }),
      ]),
    ).toEqual(["blake", "zoe"]);
  });

  test("every class has a label", () => {
    expect(mimeClassLabel("image")).toBe("Images");
    expect(mimeClassLabel("archive")).toBe("Archives");
    expect(mimeClassLabel("other")).toBe("Other");
  });
});
