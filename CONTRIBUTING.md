# Contributing to StorageScope

Thanks for contributing.

## Local Setup

```bash
cargo build
cargo fmt
cargo test
```

## Pull Request Expectations

- Keep PRs focused and small when possible.
- Add or update tests when behavior changes.
- Update docs (`README.md`, help text, etc.) when UX/CLI changes.
- For UI changes, include a screenshot or short recording.
- Ensure `cargo fmt` and `cargo test` pass before opening/updating the PR.

## AI-Assisted Contributions Policy

AI-assisted coding is welcome in this project.

If you use AI tools, please follow these rules:

- You are responsible for correctness, safety, maintainability, and licensing of submitted code.
- Disclose AI assistance in the PR description (short note is enough, e.g. `AI-assisted: yes (tool)` ).
- Do not paste secrets, credentials, or private data into prompts.
- Make sure generated code is understandable and reviewable by humans.

## Issues

When reporting bugs, include:

- OS / terminal details
- exact command used
- expected vs actual behavior
- steps to reproduce
- relevant logs/screenshots

## Style

- Rustfmt is required.
- Prefer clear, maintainable code over clever code.
- Preserve current UX conventions unless explicitly changing them.
