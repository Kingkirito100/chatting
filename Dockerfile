# Pakost Chatting: один бинарник сервера, без базы и облачных зависимостей.
FROM rust:bookworm AS build
WORKDIR /src
COPY . .
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
CMD ["sh", "-c", "exec krug-server --addr 127.0.0.1:7777 --ws-addr 0.0.0.0:${PORT:-8080} --data ${KRUG_DATA:-/data} --invite \"${KRUG_INVITE:-}\" --name \"${KRUG_NAME:-Pakost Chatting}\" --motd \"${KRUG_MOTD:-Свои люди, свои игры.}\""]
