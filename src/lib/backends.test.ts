import { describe, it, expect } from "vitest";
import { commonCredentialKeys, detectBackend } from "./backends";

describe("detectBackend", () => {
  it("recognizes s3 and b2 prefixes specifically", () => {
    expect(detectBackend("s3:s3.amazonaws.com/bucket")).toBe("s3");
    expect(detectBackend("b2:my-bucket:restic")).toBe("b2");
  });

  it("classifies other remote prefixes as other", () => {
    expect(detectBackend("sftp:user@host:/path")).toBe("other");
    expect(detectBackend("rest:http://localhost:8000/")).toBe("other");
    expect(detectBackend("azure:container:/")).toBe("other");
    expect(detectBackend("gs:bucket:/")).toBe("other");
    expect(detectBackend("rclone:remote:path")).toBe("other");
  });

  it("defaults to local for anything else", () => {
    expect(detectBackend("/home/user/backups")).toBe("local");
    expect(detectBackend("")).toBe("local");
  });

  it("is case-sensitive, matching isRemoteRepo's deliberate behavior", () => {
    expect(detectBackend("S3:bucket")).toBe("local");
    expect(detectBackend("B2:bucket:path")).toBe("local");
  });
});

describe("commonCredentialKeys", () => {
  it("names the common S3 env vars", () => {
    expect(commonCredentialKeys("s3:s3.amazonaws.com/bucket")).toBe(
      "AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY"
    );
  });

  it("names the common B2 env vars", () => {
    expect(commonCredentialKeys("b2:my-bucket:restic")).toBe("B2_ACCOUNT_ID, B2_ACCOUNT_KEY");
  });

  it("returns undefined for local, other, and unrecognized paths", () => {
    expect(commonCredentialKeys("/home/user/backups")).toBeUndefined();
    expect(commonCredentialKeys("sftp:user@host:/path")).toBeUndefined();
    expect(commonCredentialKeys("")).toBeUndefined();
  });
});
