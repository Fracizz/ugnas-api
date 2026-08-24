# Security

This is an unofficial client and handles NAS credentials, session tokens and one-shot download URLs.

- Prefer `--prompt-password` or `--password-stdin` over `--password`.
- Keep `.env`, profile credentials, sessions, captures and generated research output out of source control.
- Windows profile passwords and session tokens are protected with the current user's DPAPI key.
- CLI JSON redacts sensitive fields and URL query values by default. Use `--show-sensitive` only when the output
  will not be logged or shared.
- Use TLS when the NAS supports it. `insecure=true` disables certificate validation and should be limited to a
  trusted network.

Report suspected credential exposure privately to the repository owner. Revoke exposed sessions and share links
before sharing diagnostic output.
