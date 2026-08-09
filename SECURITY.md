# Security policy

## Reporting

Please use GitHub's private vulnerability reporting for this repository. Do not open a public issue for a vulnerability involving command execution, credential exposure, terminal injection, unsafe discovery, or history leakage.

The current supported line is 0.1.x. Expect an acknowledgement within seven days and coordinated disclosure after a fix is available.

## Threat model

Adaptive treats shell input, help/man output, documentation, provider responses, repository files, and history as untrusted data. The engine never evaluates discovered or remote content. Informational subprocesses use closed stdin, disabled pagers, short timeouts, and bounded output. Network clients use HTTPS official endpoints, timeouts, allowlisted documentation mappings, and response limits. Candidate strings are rejected when they contain terminal escapes or control delimiters.

History is sanitized before local persistence. Provider tokens are never written to cache or logs. Adaptive has no telemetry.

Suggestions are data, not commands: Adaptive inserts text into ZLE and does not invoke the resulting buffer. Users remain responsible for reviewing a command before pressing Enter.

