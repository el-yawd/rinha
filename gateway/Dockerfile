FROM rust:1.88-alpine as builder
WORKDIR /usr/src/app
RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    openssl-libs-static \
    pkgconfig
COPY . .

ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target x86_64-unknown-linux-musl

# Use scratch or minimal base
FROM alpine:latest
RUN mkdir -p /data && chmod 755 /data
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/gateway /usr/local/bin/gateway
CMD ["gateway"]
