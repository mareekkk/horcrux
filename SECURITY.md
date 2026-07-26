# Security policy

## Supported versions

| Version | Supported |
| --- | --- |
| Latest `0.1.x` release | Yes |
| Older or unreleased builds | Best effort |

Until Horcrux reaches 1.0, security fixes are released on the latest minor
series rather than backported to every earlier build.

## Reporting a vulnerability

Please use
[GitHub private vulnerability reporting](https://github.com/mareekkk/horcrux/security/advisories/new).
Do not open a public issue for a vulnerability.

Include:

- The affected Horcrux version or commit.
- The reMarkable OS and rm2display versions.
- A minimal reproduction or proof of concept.
- The likely impact.
- Any suggested mitigation.

Never include a real API key, private diary content, or another person's data.
Use synthetic examples and redact logs.

You should receive an acknowledgement within seven days. Confirmed issues will
be assessed, fixed privately when appropriate, and disclosed through a GitHub
security advisory and release notes.

## Security boundaries

Horcrux:

- Runs as root because direct display and input access require it on the
  supported tablet stack.
- Sources `/home/root/horcrux/horcrux.env`; that file must be owned by root and
  remain mode `600`.
- Sends page images and configured conversation context to the selected model
  endpoint.
- Stores conversation and journal data as unencrypted files on the tablet.
- Does not provide a network listener, hosted service, account system,
  telemetry, or automatic credential upload.

Physical access to an unlocked or SSH-accessible tablet is outside the
application's threat model.
