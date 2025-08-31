# Stage 1: Build the application
FROM rust:latest AS builder

# Install build dependencies.
# - libssl-dev is required for the `openssl` crate.
# - pkg-config is used by the `openssl` crate's build script.
RUN apt-get update && apt-get install -y libssl-dev pkg-config

WORKDIR /usr/src/armake2

# --- Build the actual application ---
# Now, copy the rest of the application's source code.
COPY . .

# Build the application in release mode.
# This will be much faster as it uses the cached dependencies from the previous step.
RUN cargo build --release
