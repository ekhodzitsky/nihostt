# Verifying nihostt releases

Every tagged release on GitHub ships attestation alongside the binary tarballs. You don't need all methods — pick the one that matches your threat model.

## 1. SHA-256 checksums (every release)

`SHA256SUMS.txt` lists the expected digest for every `*.tar.gz`. This protects against corruption in flight but **not** against a compromised GitHub release (an attacker with release access could publish matching checksums alongside tampered binaries).

```sh
gh release download v0.1.0 -R ekhodzitsky/nihostt \
    -p 'nihostt-*.tar.gz' -p 'SHA256SUMS.txt'
shasum -a 256 -c SHA256SUMS.txt
```

## 2. minisign signatures

If the maintainer's minisign key is loaded in CI, every tarball + `SHA256SUMS.txt` gets a detached `.minisig` signature. This protects against a compromised release (the attacker would also need the minisign private key).

Verify with [minisign](https://jedisct1.github.io/minisign/) or [rsign2](https://github.com/jedisct1/rsign2):

```sh
gh release download v0.1.0 -R ekhodzitsky/nihostt \
    -p '*.tar.gz' -p '*.tar.gz.minisig'
# Replace with the published public key
minisign -Vm nihostt-0.1.0-aarch64-apple-darwin.tar.gz -p nihostt.pub
```

## 3. Docker image verification

Docker images published to GHCR or Docker Hub can be verified via digest:

```sh
# Pull and inspect the image digest
docker pull ghcr.io/ekhodzitsky/nihostt:latest
docker inspect ghcr.io/ekhodzitsky/nihostt:latest --format='{{index .RepoDigests 0}}'
```

Compare the digest against the one published in the release notes.

## What to use when

| Threat | SHA256 | minisign | Docker digest |
|---|---|---|---|
| Mirror / in-flight tampering | ✅ | ✅ | ✅ |
| Compromised GitHub release | ❌ | ✅ | ⚠️ only if registry is separate |
| Compromised maintainer CI token | ❌ | ✅ | ❌ |
| Rebuild reproducibility proof | ❌ | ❌ | ✅ |

For privacy-conscious deployments, verify **both** minisign and checksums — they fail independently, so it takes two compromises to forge.