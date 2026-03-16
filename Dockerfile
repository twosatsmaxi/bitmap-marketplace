# Build stage
FROM rust:1.88-bookworm AS builder

WORKDIR /app

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create stub main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true

# Copy actual source code
COPY src/ src/
COPY migrations/ migrations/

# Touch main.rs so cargo rebuilds it (not the cached stub)
RUN touch src/main.rs

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/bitmap-marketplace /usr/local/bin/
COPY --from=builder /app/migrations/ /app/migrations/

EXPOSE 3000

CMD ["bitmap-marketplace"]
