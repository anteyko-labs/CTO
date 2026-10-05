# Сборка клиента
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# Сборка сервера (запросы проверяются по данным из server/.sqlx)
FROM rust:1-bookworm AS server
WORKDIR /server
ENV SQLX_OFFLINE=true
COPY server/ ./
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /app avtodom
WORKDIR /app
COPY --from=server /server/target/release/avtodom-server /app/avtodom-server
COPY --from=web /web/dist /app/web
ENV BIND_ADDR=0.0.0.0:8080 WEB_DIR=/app/web
USER avtodom
EXPOSE 8080
CMD ["/app/avtodom-server"]
