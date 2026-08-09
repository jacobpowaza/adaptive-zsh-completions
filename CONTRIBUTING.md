# Contributing

Thanks for improving Adaptive. Bug reports with a minimal command buffer, sanitized help output, platform, and `adaptive doctor` output are especially useful. Never include tokens, private repository names, or raw shell history.

## Development workflow

1. Open an issue for architectural changes. Small parser/provider fixes can go directly to a pull request.
2. Create a focused branch and add a regression test before the fix where practical.
3. Run `make test`, `make check`, and `./tests/install.sh`.
4. Explain security/privacy implications in the pull request.

Keep command-specific behavior declarative and small. A provider is appropriate for dynamic data; ordinary flags and subcommands belong in generic discovery. New network providers must use official endpoints, strict timeouts and response limits, local TTL caches, output sanitization, and offline degradation.

All contributions must follow the [Code of Conduct](CODE_OF_CONDUCT.md) and are accepted under the MIT license.

