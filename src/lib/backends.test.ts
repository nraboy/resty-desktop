import { describe, it, expect } from "vitest";
import {
  commonCredentialKeys,
  detectBackend,
  hasBrokenRestUserinfo,
  hasInlineRestUserinfo,
  insecureTlsConflict,
  supportsInsecureTls,
} from "./backends";

describe("detectBackend", () => {
  it("recognizes s3 and b2 prefixes specifically", () => {
    expect(detectBackend("s3:s3.amazonaws.com/bucket")).toBe("s3");
    expect(detectBackend("b2:my-bucket:restic")).toBe("b2");
  });

  it("recognizes rest: specifically", () => {
    expect(detectBackend("rest:http://localhost:8000/")).toBe("rest");
    expect(detectBackend("rest:https://host/")).toBe("rest");
  });

  it("classifies other remote prefixes as other", () => {
    expect(detectBackend("sftp:user@host:/path")).toBe("other");
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
    expect(detectBackend("REST:https://host/")).toBe("local");
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

  it("names the REST auth env vars", () => {
    expect(commonCredentialKeys("rest:https://host/")).toBe(
      "RESTIC_REST_USERNAME, RESTIC_REST_PASSWORD"
    );
  });

  it("returns undefined for local, other, and unrecognized paths", () => {
    expect(commonCredentialKeys("/home/user/backups")).toBeUndefined();
    expect(commonCredentialKeys("sftp:user@host:/path")).toBeUndefined();
    expect(commonCredentialKeys("")).toBeUndefined();
  });
});

describe("REST userinfo detectors", () => {
  const cases: [string, boolean, boolean][] = [
    // path, broken, inline
    ["rest:https://user:pass/word@h.tld", true, true],
    ["rest:https://user:pass@h.tld/", false, true],
    ["rest:https://user:pass%2Fword@h.tld", false, true],
    ["rest:https://user@h.tld/", false, true],
    ["rest:https://h.tld/", false, false],
    ["rest:http://h.tld:8000/", false, false],
    ["rest:", false, false],
    ["rest:https://", false, false],
    ["sftp:user@host:/path", false, false],
    ["s3:bucket/a@b", false, false],
    ["/local/path", false, false],
    // Mixed/uppercase scheme must still be stripped, not mistaken for userinfo.
    ["rest:HTTPS://user:pass@h.tld/", false, true],
    ["rest:Http://user:pass/word@h.tld", true, true],
    // A bare "@" with nothing before it has no real username or password.
    ["rest:https://@h.tld/", false, false],
  ];

  it.each(cases)("hasBrokenRestUserinfo(%s) === %s", (path, broken) => {
    expect(hasBrokenRestUserinfo(path)).toBe(broken);
  });

  it.each(cases)("hasInlineRestUserinfo(%s) === %s", (path, _broken, inline) => {
    expect(hasInlineRestUserinfo(path)).toBe(inline);
  });
});

describe("supportsInsecureTls", () => {
  it("is true only for rest:https:// URLs", () => {
    expect(supportsInsecureTls("rest:https://nas:8000/repo")).toBe(true);
    expect(supportsInsecureTls("  REST:HTTPS://nas/")).toBe(true);
    expect(supportsInsecureTls("rest:http://nas:8000/repo")).toBe(false);
    expect(supportsInsecureTls("s3:s3.amazonaws.com/bucket")).toBe(false);
    expect(supportsInsecureTls("/home/user/repo")).toBe(false);
  });
});

describe("insecureTlsConflict", () => {
  const insecureRest = { path: "rest:https://nas:8000/a", insecureTls: true };
  const verifiedRest = { path: "rest:https://other:8000/b", insecureTls: false };
  const local = { path: "/backups/repo", insecureTls: false };

  it("flags a pair where the flag would weaken a TLS-verifying repo, in either order", () => {
    expect(insecureTlsConflict(insecureRest, verifiedRest)).toBe(true);
    expect(insecureTlsConflict(verifiedRest, insecureRest)).toBe(true);
  });

  it("allows both-flagged, neither-flagged and non-TLS counterparts", () => {
    expect(insecureTlsConflict(insecureRest, { ...verifiedRest, insecureTls: true })).toBe(false);
    expect(insecureTlsConflict(verifiedRest, { ...verifiedRest })).toBe(false);
    expect(insecureTlsConflict(insecureRest, local)).toBe(false);
    expect(insecureTlsConflict(insecureRest, { path: "sftp:u@h:/r", insecureTls: false })).toBe(false);
    expect(insecureTlsConflict(insecureRest, { path: "rest:http://nas/r", insecureTls: false })).toBe(false);
  });
});
