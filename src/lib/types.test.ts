import { describe, it, expect } from "vitest";
import { isRemoteRepo } from "./types";

describe("isRemoteRepo", () => {
  describe("recognized remote prefixes", () => {
    it.each(["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"])(
      "returns true for %s prefix",
      (prefix) => {
        expect(isRemoteRepo(`${prefix}my-bucket`)).toBe(true);
        expect(isRemoteRepo(`${prefix}`)).toBe(true);
      }
    );

    it("handles realistic remote paths", () => {
      expect(isRemoteRepo("s3:s3.amazonaws.com/my-backup-bucket")).toBe(true);
      expect(isRemoteRepo("sftp:user@host:/backups/repo")).toBe(true);
      expect(isRemoteRepo("rest:http://localhost:8000/")).toBe(true);
      expect(isRemoteRepo("b2:my-bucket:restic-backup")).toBe(true);
      expect(isRemoteRepo("rclone:myremote:bucket/path")).toBe(true);
    });
  });

  describe("local paths", () => {
    it("returns false for absolute local paths", () => {
      expect(isRemoteRepo("/home/user/backups")).toBe(false);
      expect(isRemoteRepo("/Volumes/external/restic")).toBe(false);
      expect(isRemoteRepo("/")).toBe(false);
    });

    it("returns false for relative paths", () => {
      expect(isRemoteRepo("./backups")).toBe(false);
      expect(isRemoteRepo("backups")).toBe(false);
      expect(isRemoteRepo("../repo")).toBe(false);
    });

    it("returns false for paths that contain a colon but not as a prefix", () => {
      expect(isRemoteRepo("/path/with:colon")).toBe(false);
      expect(isRemoteRepo("host:path")).toBe(false);
    });

    it("returns false for empty string", () => {
      expect(isRemoteRepo("")).toBe(false);
    });
  });

  describe("case sensitivity", () => {
    it("does not recognize uppercase prefixes", () => {
      expect(isRemoteRepo("S3:bucket")).toBe(false);
      expect(isRemoteRepo("SFTP:host")).toBe(false);
      expect(isRemoteRepo("B2:bucket")).toBe(false);
    });
  });
});
