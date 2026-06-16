# Stage 1: Build (or use pre-built binary)
FROM rust:latest AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

COPY src/ ./src/

# Force rebuild by touching every source file
RUN find src/ -name '*.rs' -exec touch {} + && cargo build --release

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

EXPOSE 8080

CMD ["/app/omniagent"]
