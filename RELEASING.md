# Releasing StorageScope

This is a practical release checklist for GitHub Releases.

## 1. Prepare

- Make sure local branch is up to date.
- Run:

```bash
cargo fmt
cargo test
```

## 2. Bump Version

Update `version` in `Cargo.toml`.

## 3. Commit and Tag

```bash
git add -A
git commit -m "release: vX.Y.Z"
git tag -a vX.Y.Z -m "StorageScope vX.Y.Z"
```

## 4. Push

```bash
git push origin main
git push origin vX.Y.Z
```

## 5. Create GitHub Release

- Open repository on GitHub.
- Go to **Releases** -> **Draft a new release**.
- Select tag `vX.Y.Z`.
- Title: `vX.Y.Z`.
- Add release notes (highlights, fixes, known issues).
- Publish release.

## Optional: Attach Binaries

If you build artifacts, attach them to the GitHub release.

## Optional: Publish to crates.io Later

```bash
cargo publish
```

Before publishing, verify package contents:

```bash
cargo package --list
```
