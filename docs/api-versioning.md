# API Versioning Policy

nihostt follows **semantic versioning** for releases and **additive-only** evolution for public APIs.

## Versioning Strategy

- **URL path versioning**: `/v1/transcribe`, `/v1/ws`, etc.
- **WebSocket protocol version**: `PROTOCOL_VERSION = "1.0"` sent in the `ready` message.
- **Current API version**: `v1` (stable).

## Backward Compatibility Rules

1. **Additive only** — new fields may be added to JSON responses, but existing fields are never removed or renamed.
2. **New endpoints** — new routes may be added under `/v1/*` or introduced as `/v2/*` without breaking `/v1/*`.
3. **Optional features** — new response fields use `skip_serializing_if = "Option::is_none"` so older clients ignore them.
4. **Deprecation cycle** — a deprecated field is kept for at least **2 minor releases** before removal.
5. **Breaking changes** — require a new API version (e.g. `/v2/ws`) or a new message type, never a modification of existing types.

## Client Guidance

- Ignore unknown JSON fields (forward compatibility).
- Check `protocol_version` in the WebSocket `ready` message; disconnect gracefully if the major version differs.
- The `confidence` field in REST responses is optional and may be absent for silent audio.

## Stability Guarantees

| Version | Status | Since | Guarantee |
|---|---|---|---|
| v1 | **Stable** | 0.1.0 | Fully supported; additive changes only |
| v2 | — | — | Not yet planned |
