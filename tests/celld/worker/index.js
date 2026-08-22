// One Todo aggregate per Durable Object instance.
// Worker: GET/PUT /todo/:id, POST /todo/:id/complete
// Cell address: env.TODO.idFromName(id)  →  shard = todo id (PCH-REQ-003)

export class TodoCell {
  constructor(state, _env) {
    this.state = state;
    this.sql = state.storage.sql;
    this.sql.exec(`
      CREATE TABLE IF NOT EXISTS todo (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        status TEXT NOT NULL
      )
    `);
  }

  async fetch(request) {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    // ["todo", id] or ["todo", id, "complete"]
    const id = parts[1];
    if (!id) {
      return json({ error: "missing todo id" }, 400);
    }

    if (request.method === "GET" && parts.length === 2) {
      const row = firstRow(this.sql.exec("SELECT id, title, status FROM todo WHERE id = ?", id));
      if (!row) {
        return json({ error: "not found", id }, 404);
      }
      return json(row, 200);
    }

    if (request.method === "PUT" && parts.length === 2) {
      const body = await request.json().catch(() => ({}));
      const title = typeof body.title === "string" ? body.title.trim() : "";
      if (!title) {
        return json({ error: "title required" }, 400);
      }
      const existing = firstRow(this.sql.exec("SELECT id FROM todo WHERE id = ?", id));
      if (existing) {
        return json({ error: "already exists", id }, 409);
      }
      this.sql.exec(
        "INSERT INTO todo (id, title, status) VALUES (?, ?, ?)",
        id,
        title,
        "open",
      );
      return json({ id, title, status: "open" }, 201);
    }

    if (request.method === "POST" && parts[2] === "complete") {
      const row = firstRow(
        this.sql.exec("SELECT id, title, status FROM todo WHERE id = ?", id),
      );
      if (!row) {
        return json({ error: "not found", id }, 404);
      }
      if (row.status !== "open") {
        return json({ error: "not open", id, status: row.status }, 422);
      }
      this.sql.exec("UPDATE todo SET status = ? WHERE id = ?", "completed", id);
      return json({ id, title: row.title, status: "completed" }, 200);
    }

    return json({ error: "not found" }, 404);
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/" || url.pathname === "/health") {
      return new Response("distributed todo cell\n", { status: 200 });
    }
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] !== "todo" || !parts[1]) {
      return new Response("todo cell. PUT/GET /todo/:id  POST /todo/:id/complete\n", {
        status: 404,
      });
    }
    const id = parts[1];
    const stub = env.TODO.get(env.TODO.idFromName(id));
    return stub.fetch(request);
  },
};

function firstRow(cursor) {
  for (const row of cursor) {
    return row;
  }
  return null;
}

function json(body, status) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}
