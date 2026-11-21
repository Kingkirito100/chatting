# Vercel: mobile web client

The repository's mobile client is a static HTML app in `mobile/`. The root
`vercel.json` publishes that directory as the site output. Vercel hosts the
client; the Rust server must remain on a host that supports persistent
WebSocket connections and durable storage.

The configured Rust server is `wss://pakost-chatting.onrender.com`; the page
uses it by default outside local development. You can override it with the
**Сервер** field or the `?server=wss://…` query parameter. HTTPS pages require
`wss://`.

Vercel deployment can be linked to this GitHub repository in the Vercel
Dashboard; each push then builds a new static client deployment.
