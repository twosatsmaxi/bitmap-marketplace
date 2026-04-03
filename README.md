# Bitmap Marketplace

A Bitcoin ordinals marketplace with PSBT-based atomic swaps and BIP-322 authentication.

## Quick Start

```bash
# Setup environment
cp .env.example .env
# Edit .env with your settings (see Configuration below)

# Run database migrations
# (Handled automatically on startup)

# Start the server
cargo run
```

## Configuration

Copy `.env.example` to `.env` and configure:

### Required

| Variable | Description | Example |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection string | `postgres://user:pass@localhost:5432/bitmap_marketplace` |
| `JWT_SECRET` | **NEW**: 32-byte hex secret for JWT signing | Generate with `openssl rand -hex 32` |
| `MARKETPLACE_SECRET_KEY` | 32-byte hex for PSBT signing | `openssl rand -hex 32` |

### Optional

| Variable | Description | Default |
|----------|-------------|---------|
| `FRONTEND_URL` | **NEW**: CORS origin restriction (production) | (unset = allow any) |
| `ALLOWED_ADDRESS_NETWORK` | **NEW**: Which network addresses auth accepts (`bitcoin`/`testnet`/`regtest`) | `bitcoin` |
| `BITCOIN_RPC_URL` | Bitcoin Core JSON-RPC endpoint | `http://127.0.0.1:8332` |
| `BITCOIN_NETWORK` | Network (mainnet/testnet/regtest) | `regtest` |
| `ORD_URL` | ord indexer URL | `http://127.0.0.1:80` |
| `PORT` | HTTP server port | `3000` |

---

## Security Configuration

### 1. JWT Secret (Required)

Generate a strong random secret:

```bash
openssl rand -hex 32
```

Add to `.env`:
```bash
JWT_SECRET=a3f5c8...64-char-hex...9e2b1d
```

**Security**: Used for signing/verifying authentication tokens. Must be kept secret and rotated if compromised.

### 2. CORS Origin Restriction (Production)

In production, restrict CORS to your frontend domain:

```bash
FRONTEND_URL=https://your-marketplace-frontend.com
```

In development, leave unset to allow any origin.

### 3. Rate Limiting

Auth endpoints (`/api/auth/*`) have stricter rate limiting:
- 3 requests/second, 10 burst

General API:
- 10 requests/second, 30 burst

---

## Authentication Flow

The marketplace uses BIP-322 signature verification for wallet authentication:

```
1. GET /api/auth/challenge?address=<btc-address>
   → Returns challenge message with nonce
   
2. POST /api/auth/connect
   {
     "paymentAddress": "bc1p...",
     "ordinalsAddress": "bc1p...",
     "signature": "<bip322-signature>",
     "message": "<challenge-message>",
     "nonce": "<nonce>"
   }
   → Returns JWT token + profile
```

**Security features:**
- Bitcoin addresses are validated (format, checksum, network)
- Challenges expire after 10 minutes
- BIP-322 signatures verified against stored challenge
- LRU-capped challenge store (max 10,000 entries) prevents DoS

---

## API Overview

| Endpoint | Auth | Description |
|----------|------|-------------|
| `GET /api/auth/challenge` | No | Get authentication challenge |
| `POST /api/auth/connect` | No | Connect wallet, get JWT |
| `GET /api/auth/profile` | Yes | Get current profile |
| `DELETE /api/auth/wallets/:address` | Yes | Remove wallet |
| `GET /api/bitmaps` | No | List bitmaps |
| `GET /api/listings` | No | List active listings |
| `POST /api/listings` | Yes | Create listing |
| `GET /api/offers` | Yes | List offers |
| `POST /api/offers/:id/accept` | Yes | Accept offer |
| `WS /ws` | No | WebSocket events |

---

## Security Audit History

### 2026-04-03 - Auth Flow Hardening

Fixed critical and high-severity security issues:

| Issue | Severity | Fix |
|-------|----------|-----|
| JWT accepted "none" algorithm | **P0** | Explicit `Algorithm::HS256` requirement |
| Permissive CORS | **P1** | Origin restriction via `FRONTEND_URL` |
| No address validation | **P1** | Format + network validation in auth flow |
| Unbounded challenge store | **P1** | LRU cache (10k max) for DoS protection |
| TOCTOU timing issues | **P2** | Single timestamp capture |
| Signature verification fragile | **P2** | Verify against stored challenge directly |

**Required actions for existing deployments:**
1. Add `JWT_SECRET` to `.env` (see Configuration)
2. Set `FRONTEND_URL` in production
3. Restart service

---

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│   Client    │────▶│  Axum API    │────▶│ PostgreSQL  │
│  (Wallet)   │     │              │     │             │
└─────────────┘     ├──────────────┤     └─────────────┘
       │            │ JWT Auth     │            ▲
       │            │ BIP-322      │            │
       │            │ PSBT Builder │     ┌──────┴──────┐
       │            └──────────────┘     │  Bitcoin    │
       │                    │            │   Core RPC  │
       └────────────────────┴────────────┴─────────────┘
                        WebSocket Events
```

---

## Development

```bash
# Run tests
cargo test

# Check formatting
cargo fmt -- --check

# Build release
cargo build --release
```

## Docker

```bash
docker-compose up -d
```

Services:
- `bitmap-marketplace`: API server (port 3000)
- `postgres`: Database

---

## License

MIT
