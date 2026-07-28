# Windows Authenticode signing

Clipboard Transformer currently publishes unsigned Windows EXE, ZIP, and MSI
artifacts. GitHub attestations and SHA-256 sidecars prove release provenance,
but they do not replace an Authenticode signature for SmartScreen or managed
Windows policy.

## Provider decision

Choose one provider before changing the workflow because each service uses a
different signing command and credential model.

| Provider | Cost and eligibility | CI characteristics | Trade-off |
| --- | --- | --- | --- |
| [SignPath Foundation](https://signpath.org/terms.html) | Free for accepted OSS projects | Managed signing pipeline; each release needs approval | Certificate publisher is `SignPath Foundation`, and acceptance requires a released, documented, reputable OSS project with verifiable builds |
| [Azure Artifact Signing](https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options) | Approximately USD 9.99/month; organizations in the US, Canada, EU, or UK, and individuals in the US or Canada | No USB token; direct CI integration | Identity and region eligibility must be approved first |
| Traditional OV certificate | Microsoft estimates roughly USD 150-300/year, but actual CA offers vary | Private key must live in a hardware token, HSM, or cloud HSM | Worldwide option; a USB token is awkward on GitHub-hosted runners |

Do not buy an EV certificate solely to bypass SmartScreen. Microsoft documents
that EV has used the same reputation-building behavior as OV since 2024.
Self-signed certificates are for local testing or managed enterprises that
explicitly distribute the trust root, not public downloads.

Start by applying to SignPath Foundation. If its publisher identity or approval
workflow is unsuitable, use Azure Artifact Signing when region-eligible;
otherwise obtain an OV certificate through a CA with a cloud signing option.

## Required signing order

Authenticode covers file bytes, so packaging order is part of the security
contract:

1. Build `Clipboard Transformer.exe` and `clipboard-transformer.exe`.
2. Sign both EXE files with SHA-256 and an RFC 3161 timestamp.
3. Verify both EXE signatures.
4. Stage the signed EXE files and build the MSI.
5. Sign and verify the MSI.
6. Build the portable ZIP from the already signed EXE files.
7. Generate SHA-256 sidecars and GitHub attestations from the final bytes.

The ZIP itself is not Authenticode-signed. Its contents are signed, and the ZIP
continues to receive a checksum and GitHub attestation.

The current `just package-windows-msi` recipe builds and packages in one call.
Signing therefore requires splitting the release workflow at
`prepare-package-windows`: build and stage, sign the two EXE files, invoke
cargo-packager, sign the MSI, and only then stage release assets. Do not add a
provider-specific command until the provider account has been selected and
validated.

## Verification

Run these checks on every final artifact:

```powershell
signtool verify /pa /all /v ".\Clipboard Transformer.exe"
signtool verify /pa /all /v ".\clipboard-transformer.exe"
signtool verify /pa /all /v ".\clipboard-transformer-<version>-x86_64.msi"
```

Also inspect the timestamp and publisher:

```powershell
Get-AuthenticodeSignature ".\Clipboard Transformer.exe" | Format-List
Get-AuthenticodeSignature ".\clipboard-transformer.exe" | Format-List
Get-AuthenticodeSignature ".\clipboard-transformer-<version>-x86_64.msi" |
  Format-List
```

The status must be `Valid`. Timestamping is mandatory so signatures remain
valid after certificate expiry. A valid new certificate improves identity and
enterprise compatibility but does not guarantee that SmartScreen warnings
disappear immediately; reputation accumulates over time.
