# Stage 1: Build
FROM rust:latest AS builder

WORKDIR /app

# Copy only Cargo.toml and Cargo.lock first for dependency caching
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

# Copy actual source
COPY src/ ./src/

# Build the actual binary
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/omniagent /app/omniagent

USER root

CMD ["/app/omniagent"]
