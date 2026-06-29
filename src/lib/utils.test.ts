import { describe, it, expect } from "vitest";
import { needsFullDiskAccess } from "./utils";

describe("needsFullDiskAccess", () => {
  describe("~/Library paths", () => {
    it("matches /Library at the root", () => {
      expect(needsFullDiskAccess("/Library")).toBe(true);
    });

    it("matches /Library/ with trailing slash", () => {
      expect(needsFullDiskAccess("/Library/")).toBe(true);
    });

    it("matches /Library subdirectories", () => {
      expect(needsFullDiskAccess("/Library/Keychains")).toBe(true);
      expect(needsFullDiskAccess("/Users/alice/Library")).toBe(true);
      expect(needsFullDiskAccess("/Users/alice/Library/Mail")).toBe(true);
    });

    it("does not match paths where Library is not a standalone segment", () => {
      expect(needsFullDiskAccess("/Users/alice/MyLibrary")).toBe(false);
      expect(needsFullDiskAccess("/Users/alice/MyLibraryFiles")).toBe(false);
    });
  });

  describe("/System paths", () => {
    it("matches /System exactly", () => {
      expect(needsFullDiskAccess("/System")).toBe(true);
    });

    it("matches /System/ with trailing slash", () => {
      expect(needsFullDiskAccess("/System/")).toBe(true);
    });

    it("matches /System subdirectories", () => {
      expect(needsFullDiskAccess("/System/Library")).toBe(true);
      expect(needsFullDiskAccess("/System/Volumes")).toBe(true);
    });

    it("does not match paths that merely start with System", () => {
      expect(needsFullDiskAccess("/Systems")).toBe(false);
      expect(needsFullDiskAccess("/System2")).toBe(false);
    });
  });

  describe("/private paths", () => {
    it("matches /private exactly", () => {
      expect(needsFullDiskAccess("/private")).toBe(true);
    });

    it("matches /private subdirectories", () => {
      expect(needsFullDiskAccess("/private/var")).toBe(true);
      expect(needsFullDiskAccess("/private/etc")).toBe(true);
    });

    it("does not match paths that merely contain private", () => {
      expect(needsFullDiskAccess("/not-private")).toBe(false);
      expect(needsFullDiskAccess("/Users/alice/private-files")).toBe(false);
    });
  });

  describe("/var paths", () => {
    it("matches /var exactly", () => {
      expect(needsFullDiskAccess("/var")).toBe(true);
    });

    it("matches /var subdirectories", () => {
      expect(needsFullDiskAccess("/var/log")).toBe(true);
      expect(needsFullDiskAccess("/var/db")).toBe(true);
    });

    it("does not match paths that merely contain var", () => {
      expect(needsFullDiskAccess("/vars")).toBe(false);
      expect(needsFullDiskAccess("/Users/alice/var-backup")).toBe(false);
    });
  });

  describe("normal user paths", () => {
    it("returns false for common home directory paths", () => {
      expect(needsFullDiskAccess("/Users/alice/Documents")).toBe(false);
      expect(needsFullDiskAccess("/Users/alice/Desktop")).toBe(false);
      expect(needsFullDiskAccess("/Users/alice/Downloads")).toBe(false);
      expect(needsFullDiskAccess("/home/alice")).toBe(false);
    });

    it("returns false for other system paths", () => {
      expect(needsFullDiskAccess("/usr/local")).toBe(false);
      expect(needsFullDiskAccess("/opt")).toBe(false);
      expect(needsFullDiskAccess("/Applications")).toBe(false);
    });

    it("returns false for empty string", () => {
      expect(needsFullDiskAccess("")).toBe(false);
    });
  });
});
