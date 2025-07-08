FROM rust:slim-buster as builder
WORKDIR /usr/src/app
COPY . .
RUN cargo build --release

FROM debian:buster-slim
COPY --from=builder /usr/src/app/target/release/rinha /usr/local/bin/rinha
CMD ["rinha"]
