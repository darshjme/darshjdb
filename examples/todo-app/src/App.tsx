import React, { useState } from "react";
import { useQuery, useMutation } from "@darshjdb/react";

interface Todo {
  id: string;
  title: string;
  done: boolean;
  createdAt: number;
}

export function App() {
  const [title, setTitle] = useState("");

  // Live query — automatically updates when data changes
  const { data: todos, isLoading } = useQuery<Todo>({
    collection: "todos",
    orderBy: [{ field: "createdAt", direction: "desc" }],
  });

  const { mutate } = useMutation();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim()) return;

    await mutate({
      type: "insert",
      collection: "todos",
      data: { title: title.trim(), done: false, createdAt: Date.now() },
    });
    setTitle("");
  };

  if (isLoading) {
    return <p>Loading...</p>;
  }

  const done = todos.filter((t) => t.done).length;

  return (
    <div style={{ maxWidth: 480, margin: "40px auto", fontFamily: "system-ui" }}>
      <h1>DarshJDB Todos</h1>
      <p style={{ color: "#666" }}>
        {todos.length} total, {done} completed
      </p>

      <form onSubmit={handleSubmit} style={{ display: "flex", gap: 8, marginBottom: 16 }}>
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="What needs to be done?"
          style={{ flex: 1, padding: "8px 12px", fontSize: 16, borderRadius: 6, border: "1px solid #ddd" }}
        />
        <button
          type="submit"
          style={{ padding: "8px 16px", fontSize: 16, borderRadius: 6, background: "#000", color: "#fff", border: "none", cursor: "pointer" }}
        >
          Add
        </button>
      </form>

      <ul style={{ listStyle: "none", padding: 0 }}>
        {todos.map((todo) => (
          <li
            key={todo.id}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 0",
              borderBottom: "1px solid #eee",
            }}
          >
            <input
              type="checkbox"
              checked={todo.done}
              onChange={() =>
                mutate({
                  type: "update",
                  collection: "todos",
                  id: todo.id,
                  data: { done: !todo.done },
                })
              }
            />
            <span
              style={{
                flex: 1,
                textDecoration: todo.done ? "line-through" : "none",
                color: todo.done ? "#999" : "#000",
              }}
            >
              {todo.title}
            </span>
            <button
              onClick={() =>
                mutate({ type: "delete", collection: "todos", id: todo.id })
              }
              style={{ background: "none", border: "none", color: "#999", cursor: "pointer", fontSize: 18 }}
            >
              x
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
