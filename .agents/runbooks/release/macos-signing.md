# macOS Signing and Notarization (Developer ID)

Working reference for producing the signed and notarized macOS release
artifacts tracked in `TODO.md`. cargo-packager signs every Mach-O nested in the
app, signs the app bundle, creates the DMG, and signs the DMG. Its current
built-in notarization path only submits the app bundle; it does not submit or
staple the DMG. Apple permits shipping an unnotarized disk image containing an
already notarized and stapled app, but also supports notarizing a container and
processing its nested contents. The custom release job uses the latter
workflow: it owns the shared temporary keychain, signs the standalone CLI with
the same identity, submits the outermost DMG once, staples the resulting
tickets to both the DMG and nested app, and submits a temporary ZIP containing
the standalone CLI so it gets an online notarization ticket. This document
covers the credentials and verification for that complete pipeline.

## What the pipeline consumes

| Purpose | Environment variable | What it is |
| --- | --- | --- |
| Signing (CI) | `APPLE_CERTIFICATE` | base64 of the exported `.p12` |
| Signing (CI) | `APPLE_CERTIFICATE_PASSWORD` | password chosen at `.p12` export |
| Signing identity | derived in CI | `Developer ID Application` identity read from the imported `.p12` |
| Signing (local) | `APPLE_SIGNING_IDENTITY` | identity string, cert in login keychain |
| Notarization (CI) | `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH` | App Store Connect API key id, issuer id, path to the `.p8` |
| Notarization, local convenience | `APPLE_KEYCHAIN_PROFILE` | profile saved by `xcrun notarytool store-credentials` |

Signing and notarization credentials are independent: the certificate proves
who built the artifact; the notarization credential lets Apple's service scan
it. You need one from each group.

## 1. Create the Developer ID Application certificate

One certificate, valid 5 years, reused for every release. Only the **Account
Holder** role can create Developer ID certificates — other team roles get a
greyed-out option.

Pick the correct type: **Developer ID Application**. Not "Developer ID
Installer" (that signs `.pkg` installers only) and not "Apple
Development/Distribution" (App Store only).

**Path A — Xcode (fewer steps).** Xcode → Settings → Accounts → select the
team → Manage Certificates… → "+" → Developer ID Application. The private key
is generated straight into the login keychain. Done; skip to verification.

**Path B — web portal.**

1. Generate a CSR locally: Keychain Access → menu Keychain Access →
   Certificate Assistant → *Request a Certificate From a Certificate
   Authority…* Enter the Apple ID email and any recognizable Common Name,
   choose **Saved to disk**. This writes
   `CertificateSigningRequest.certSigningRequest` **and creates the private
   key in the login keychain of this machine** — the machine matters.
2. <https://developer.apple.com/account> → Certificates, Identifiers &
   Profiles → Certificates → "+" → Developer ID Application. When asked for a
   profile type, choose the G2 Sub-CA (Xcode 11.4.1 or later) variant.
3. Upload the CSR, download `developerID_application.cer`, double-click it to
   import into the login keychain, where it pairs with the private key from
   step 1.

**Verify:**

```sh
security find-identity -v -p codesigning
# expect: 1) <hash> "Developer ID Application: <Name> (<TEAMID>)"
```

If it reports 0 valid identities or "certificate not trusted", the Apple
intermediate is missing: install **Developer ID Certification Authority (G2)**
from <https://www.apple.com/certificateauthority/> and re-check.

The **Team ID** is the 10-character code in the identity's parentheses; it is
also shown at <https://developer.apple.com/account> under Membership details.

## 2. Export the `.p12` for CI

1. Keychain Access → login keychain → *My Certificates* → the
   "Developer ID Application: …" entry must have a disclosure triangle with a
   private key beneath it (no key = wrong machine, see step 1 path B).
2. Select the certificate row → File → Export Items… → format **.p12** → set
   a strong export password.
3. Encode and store:

   ```sh
   base64 -i DeveloperID.p12 | pbcopy
   ```

4. GitHub repo → Settings → Secrets and variables → Actions → new secrets:
   - `APPLE_CERTIFICATE` — the base64 output;
   - `APPLE_CERTIFICATE_PASSWORD` — the export password.

Keep the `.p12` itself in a password manager. **Apple never stores the private
key** — the portal can only re-issue a new certificate, not recover this one.
The release job imports the `.p12` into an ephemeral CI keychain, exposes that
identity to both `codesign` and cargo-packager, then deletes the keychain even
when an earlier step fails. CI runners need no pre-installed identity.

## 3. Notarization credentials

Notarization is part of Developer ID distribution outside the Mac App Store.
Neither authentication option publishes the application or creates an App
Store listing.

**Local alternative: Apple ID and app-specific password.** This avoids creating
an App
Store Connect API key. It requires 2FA on the Apple ID. Generate the password
at <https://account.apple.com> → Sign-In and Security → App-Specific Passwords.
Use the generated password, never the real account password. The release
workflow intentionally uses API-key authentication instead, so CI has one
notarization path.

You can validate and save these credentials locally:

```sh
xcrun notarytool store-credentials clipboard-transformer \
  --apple-id you@example.com \
  --team-id TEAMID \
  --password xxxx-xxxx-xxxx-xxxx
```

**CI: App Store Connect API key.** Not tied to a personal login session,
has no 2FA prompts, and is independently revocable.

1. <https://appstoreconnect.apple.com> → Users and Access → Integrations →
   App Store Connect API → **Team Keys** → Generate API Key.
2. Role: **Developer** is sufficient for notarization.
3. Download the `.p8`. **Apple offers this download exactly once** — store the
   file in a password manager immediately.
4. Record the **Key ID** (shown on the key's row) and the **Issuer ID** (UUID
   at the top of the Team Keys page).

Store the `.p8` content as a secret (e.g. `APPLE_API_KEY_P8`) and materialize
it in the job, since packager wants a file path:

```yaml
- name: Write notarization key
  run: printf '%s' "$APPLE_API_KEY_P8" > "$RUNNER_TEMP/apple-api-key.p8"
  env:
    APPLE_API_KEY_P8: ${{ secrets.APPLE_API_KEY_P8 }}
- name: Package
  run: just package-app
  env:
    APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
    APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
    APPLE_API_KEY: ${{ secrets.APPLE_API_KEY_ID }}
    APPLE_API_ISSUER: ${{ secrets.APPLE_API_ISSUER_ID }}
    APPLE_API_KEY_PATH: ${{ runner.temp }}/apple-api-key.p8
```

GitHub Actions secrets may contain the PEM verbatim. Secret systems that
require a single-line value can store `APPLE_API_KEY_P8` as base64 instead:

```sh
base64 -i AuthKey_XXXXXXXXXX.p8 | tr -d '\n' | pbcopy
```

The release workflow accepts either the original PEM or this base64 form and
validates the decoded header before invoking `notarytool`.

**Local convenience:** save a keychain profile once and point packager at it.
For the API key:

```sh
xcrun notarytool store-credentials clipboard-transformer \
  --key /path/to/AuthKey_<KEYID>.p8 \
  --key-id <KEYID> \
  --issuer <ISSUERID>
```

Or use an Apple ID and app-specific password:

```sh
xcrun notarytool store-credentials clipboard-transformer \
  --apple-id you@example.com --team-id TEAMID --password xxxx-xxxx-xxxx-xxxx
export APPLE_KEYCHAIN_PROFILE=clipboard-transformer
```

The profile replaces notarization credential flags only. Code signing still
needs the Developer ID Application certificate and private key in a Keychain
visible to `codesign`, or a temporary CI keychain created from the `.p12`.
cargo-packager 0.11.8 reads `APPLE_KEYCHAIN_PROFILE` before its Apple ID and
API-key environment-variable groups.

## 4. Entitlements — required, not optional, for this app

packager always signs binaries with `--options runtime` (hardened runtime),
and the hardened runtime forbids runtime-created executable memory. Wasmtime
JIT-compiles plugin modules into exactly such memory, and it does **not**
allocate it with `MAP_JIT`, so `com.apple.security.cs.allow-jit` does not
help: a signed build without
`com.apple.security.cs.allow-unsigned-executable-memory` dies with
`SIGKILL (Code Signature Invalid) / CODESIGNING: Invalid Page` on the first
plugin call while the rest of the app appears healthy. Verified empirically
on 2026-07-17: `allow-jit` alone crashes,
`allow-unsigned-executable-memory` alone works.

`package/macos/entitlements.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
  <true/>
</dict>
</plist>
```

`Packager.toml`:

```toml
[macos]
entitlements = "package/macos/entitlements.plist"
# signingIdentity = "Developer ID Application: <Name> (<TEAMID>)"
```

## 5. Passing the signing identity

cargo-packager 0.11.8 reads `signingIdentity` only from its configuration
file. There is no environment variable and no CLI flag for it, and a raw JSON
`--config` replaces the entire configuration rather than overlaying it. Nothing
is signed when the identity is absent: both signing and notarization sit behind
`if let Some(identity)` in the app and DMG packagers, so `APPLE_CERTIFICATE`
and `APPLE_KEYCHAIN_PROFILE` alone do nothing.

The tracked `Packager.toml` therefore keeps the identity commented out, and
`just _packager-config` writes `Packager.local.toml` — the same config with
`APPLE_SIGNING_IDENTITY` substituted in — which the `package-app` and
`package-macos` recipes pass to `--config`. Without the variable they use the
tracked file unchanged and produce unsigned bundles. The generated copy stays
in the repository root because cargo-packager chdirs to the config file's
parent directory before resolving relative paths, and it is git-ignored by
`*.local.*`.

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"
just package-macos
```

The `Justfile` loads a git-ignored `.env` from the repository root, so
`APPLE_SIGNING_IDENTITY` and `APPLE_KEYCHAIN_PROFILE` can live there instead of
being exported in every shell. Keep that file down to those two: the login keychain
already holds the certificate, the notarytool profile already holds the
notarization credentials, and every `just` invocation exports the file's
contents to every subprocess it runs. In particular, an `APPLE_CERTIFICATE`
plus `APPLE_CERTIFICATE_PASSWORD` pair makes cargo-packager import that `.p12`
into a temporary keychain of its own instead of signing from the login
keychain.

CI derives the same variable from the imported `.p12` and calls the same
recipe, so the release job never rewrites a tracked file.

## 6. Verify a build

```sh
codesign --verify --strict --verbose=2 clipboard-transformer
codesign -dv --verbose=2 "target/packager/Clipboard Transformer.app"
#   expect: Authority=Developer ID Application…, flags containing "runtime"
codesign -d --entitlements - "target/packager/Clipboard Transformer.app"
#   expect: com.apple.security.cs.allow-unsigned-executable-memory
spctl --assess --type execute -vv "target/packager/Clipboard Transformer.app"
#   expect: accepted, source=Notarized Developer ID
xcrun stapler validate "Clipboard Transformer.app"
xcrun stapler validate "clipboard-transformer-<version>-<target>.dmg"
```

The app and DMG carry stapled tickets, so they can be checked offline. ZIP,
tar archives, and bare Mach-O executables cannot be stapled; Apple publishes
the CLI ticket online after accepting the temporary ZIP submission. An
`spctl --type execute` check is not a valid CI assertion for that bare
executable: it can reject valid code as not being an app. The CI job instead
requires the CLI submission to return `Accepted`, verifies its Developer ID
signature and secure timestamp, and puts those exact signed bytes in both the
Homebrew ZIP and CLI `.tar.xz`. The app and DMG retain their `spctl` and
stapler checks. All Cargo builds happen before `codesign`: rebuilding a
package target after signing can replace a file under `target/release` and
invalidate its existing signature.

Functional proof of the JIT entitlement: run the signed app with a plugin
rule configured (`plugins/gitlab-link/`) and confirm a copy is still
transformed — that exercises Wasmtime under the hardened runtime.

### Manual local notarization

Create the profile above once, then use it for status and logs:

```sh
xcrun notarytool history --keychain-profile clipboard-transformer
xcrun notarytool info <submission-id> \
  --keychain-profile clipboard-transformer
xcrun notarytool log <submission-id> \
  --keychain-profile clipboard-transformer \
  target/notary-log.json
```

To submit an app manually, ZIP the already signed bundle with `ditto`. Do not
use a generic recursive ZIP tool:

```sh
mkdir -p target/notarization
ditto -c -k --keepParent --sequesterRsrc \
  "target/packager/Clipboard Transformer.app" \
  "target/notarization/Clipboard Transformer.zip"
xcrun notarytool submit \
  "target/notarization/Clipboard Transformer.zip" \
  --keychain-profile clipboard-transformer \
  --wait \
  --timeout 30m
xcrun stapler staple -v "target/packager/Clipboard Transformer.app"
xcrun stapler validate "target/packager/Clipboard Transformer.app"
```

Submit and staple the signed DMG separately:

```sh
xcrun notarytool submit "target/packager/<name>.dmg" \
  --keychain-profile clipboard-transformer \
  --wait \
  --timeout 30m
xcrun stapler staple -v "target/packager/<name>.dmg"
xcrun stapler validate "target/packager/<name>.dmg"
```

Submit a temporary ZIP containing the separately signed CLI with
`notarytool submit`. The accepted ticket applies to the identical signed CLI
bytes later placed in the Homebrew ZIP and `.tar.xz`. Do not run `stapler` on
the ZIP or bare CLI. Also do not use `spctl --type execute` as an automated
assertion for the bare executable; require an `Accepted` submission, a valid
Developer ID signature, and a `Timestamp=` value from `codesign -dvv`.

## 7. Gotchas

- The `.p8` API key downloads exactly once; the `.p12` private key is never
  recoverable from Apple. Both belong in a password manager.
- Developer ID certificate creation is restricted to the Account Holder.
- A rejected notarization returns status `Invalid`; get the per-file reasons
  with `xcrun notarytool log <submission-id>` (plus the same credential
  flags). Common causes — missing timestamp, missing hardened runtime, or an
  unsigned nested binary — are already handled: both app and CLI are signed
  with `--timestamp` and hardened runtime before submission.
- Never commit the `.p12`, `.p8`, or any of these values; CI secrets only.
- Once releases are notarized, the Homebrew story switches to a plain cask
  and the source-build formula (and any `xattr` quarantine stripping) is
  retired — see `TODO.md`.
