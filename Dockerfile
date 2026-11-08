# Сборка сервера «Круга» — один бинарник, без базы и облачных зависимостей.
FROM rust:bookworm AS build
WORKDIR /src

# Сначала зависимости: так слой кэшируется между сборками.
COPY Cargo.toml Cargo.lock ./
COPY crates/krug-core/Cargo.toml crates/krug-core/
COPY crates/krug-server/Cargo.toml crates/krug-server/
COPY crates/krug-client/Cargo.toml crates/krug-client/
COPY crates/krug-cli/Cargo.toml crates/krug-cli/
COPY crates/krug-gui/Cargo.toml crates/krug-gui/
RUN mkdir -p crates/krug-core/src crates/krug-server/src crates/krug-client/src crates/krug-cli/src crates/krug-gui/src \
    && touch crates/krug-core/src/lib.rs crates/krug-server/src/lib.rs crates/krug-server/src/main.rs \
    && touch crates/krug-client/src/lib.rs crates/krug-cli/src/main.rs crates/krug-gui/src/main.rs \
    && cargo build --release -p krug-server --bin krug-server 2>/dev/null || true

COPY crates ./crates
COPY assets ./assets
COPY mobile ./mobile
RUN cargo build --release -p krug-server --bin krug-server

FROM debian:bookworm-slim
RUN useradd --create-home --uid 10001 krug \
    && mkdir -p /data \
    && chown krug:krug /data
COPY --from=build /src/target/release/krug-server /usr/local/bin/krug-server
USER krug
ENV KRUG_DATA=/data
VOLUME ["/data"]
EXPOSE 7777 8080

# PORT подставляют Render, Koyeb и другие PaaS. Локально это 8080.
CMD ["sh", "-c", "exec krug-server --addr 0.0.0.0:7777 --ws-addr 0.0.0.0:${PORT:-8080} --data ${KRUG_DATA:-/data} --invite \"${KRUG_INVITE:-}\" --name \"${KRUG_NAME:-Pakost Chatting}\" --motd \"${KRUG_MOTD:-Свои люди, свои игры.}\""]
