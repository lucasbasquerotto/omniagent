# OmniAgent — Development Dockerfile
# Source code is mounted as a volume; binary built on host with `cargo build --release`.
# No docker cp needed — just build then `docker compose restart omniagent`.

FROM rust:latest

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Run the pre-built release binary from the mounted target/ directory.
# Build on the host: cd /opt/workspace/omniagent && cargo build --release
# Then restart: docker compose restart omniagent
CMD ["./target/release/omniagent"]
