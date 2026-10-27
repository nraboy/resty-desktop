import {
  REST_PASSWORD_KEY,
  REST_USERNAME_KEY,
  getCredentialValue,
  setCredentialValue,
} from "../lib/backends";
import Input from "./Input";

type Props = {
  rows: [string, string][];
  onChange: (rows: [string, string][]) => void;
};

/**
 * "Server username" / "Server password" for a `rest:` repository. They are the credential rows
 * `RESTIC_REST_USERNAME` / `RESTIC_REST_PASSWORD` underneath — the same storage the generic
 * credential list uses, so existing repos show their saved login here unchanged.
 */
export default function RestServerLogin({ rows, onChange }: Props) {
  return (
    <div className="grid grid-cols-2 gap-2">
      <Input
        label="Server username"
        value={getCredentialValue(rows, REST_USERNAME_KEY)}
        onChange={(e) => onChange(setCredentialValue(rows, REST_USERNAME_KEY, e.target.value))}
      />
      <Input
        label="Server password"
        type="password"
        value={getCredentialValue(rows, REST_PASSWORD_KEY)}
        onChange={(e) => onChange(setCredentialValue(rows, REST_PASSWORD_KEY, e.target.value))}
      />
    </div>
  );
}
