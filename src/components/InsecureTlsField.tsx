import { supportsInsecureTls } from "../lib/backends";
import { InfoIcon } from "./icons";
import Tooltip from "./Tooltip";

type Props = {
  path: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
};

/**
 * "Skip TLS certificate verification" checkbox for a `rest:https://` repository. Renders
 * nothing for any other path. Ticking it is the user's explicit acknowledgement that the
 * server's identity won't be verified; the tooltip points at the safe alternative
 * (trusting the server's CA in the OS certificate store). See docs/restic.md.
 */
export default function InsecureTlsField({ path, checked, onChange }: Props) {
  if (!supportsInsecureTls(path)) return null;
  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2 text-sm text-gray-400">
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
          Skip TLS certificate verification (--insecure-tls)
        </label>
        <Tooltip
          title="Self-signed certificate?"
          content={
            <div className="space-y-2">
              <p>
                Without this option restic refuses to connect to a server whose certificate it
                can't verify, such as a NAS with a self-signed certificate.
              </p>
              <p>
                The connection stays encrypted, but restic no longer checks <em>who</em> it is
                talking to, so anyone able to intercept your network traffic could impersonate
                the server.
              </p>
              <p>
                The secure alternative is to add the server's certificate (or the CA that signed
                it) to your operating system's certificate store and leave this option off:
                Keychain Access on macOS (set it to "Always Trust"), "Trusted Root Certification
                Authorities" on Windows, or <span className="font-mono">update-ca-certificates</span> /{" "}
                <span className="font-mono">update-ca-trust</span> on Linux.
              </p>
            </div>
          }
        >
          <span
            tabIndex={0}
            role="img"
            aria-label="About skipping TLS certificate verification"
            className="cursor-help text-gray-500 hover:text-gray-300"
          >
            <InfoIcon className="w-4 h-4" />
          </span>
        </Tooltip>
      </div>
      {checked && (
        <p className="text-xs text-amber-400">
          Insecure: the server's identity will not be verified. Prefer trusting its certificate in
          your system certificate store.
        </p>
      )}
    </div>
  );
}
