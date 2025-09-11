FROM rust:1.70

WORKDIR /app
COPY . .

RUN cargo build --release --target x86_64-unknown-linux-gnu
