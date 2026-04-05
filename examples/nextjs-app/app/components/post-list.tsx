"use client";

import { useState } from "react";
import { useQuery, useMutation } from "@darshjdb/react";
import { createPost } from "../actions/posts";

interface Post {
  id: string;
  title: string;
  body: string;
  author: string;
  createdAt: number;
}

/**
 * Client Component that displays posts with real-time updates.
 *
 * Receives server-fetched data as initialPosts for instant render,
 * then subscribes to live query updates via useQuery.
 */
export function PostList({ initialPosts }: { initialPosts: Post[] }) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");

  // Live query -- subscribes to real-time updates.
  // Falls back to initialPosts until the first server push arrives.
  const { data, isLoading } = useQuery({
    collection: "posts",
    orderBy: [{ field: "createdAt", direction: "desc" }],
    limit: 20,
  });

  const { mutate, isLoading: isCreating } = useMutation();

  const posts: Post[] = data ?? initialPosts;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim()) return;

    // Option 1: Client-side mutation (optimistic, real-time)
    await mutate({
      type: "insert",
      collection: "posts",
      data: {
        title: title.trim(),
        body: body.trim(),
        author: "Demo User",
        createdAt: Date.now(),
      },
    });

    // Option 2: Server Action (uncomment to use instead)
    // await createPost(title.trim(), body.trim());

    setTitle("");
    setBody("");
  };

  return (
    <div>
      {/* New post form */}
      <form onSubmit={handleSubmit} style={formStyle}>
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Post title"
          required
          style={inputStyle}
        />
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="Write something..."
          rows={3}
          style={{ ...inputStyle, resize: "vertical" }}
        />
        <button type="submit" disabled={isCreating} style={buttonStyle}>
          {isCreating ? "Posting..." : "Publish"}
        </button>
      </form>

      {/* Post list */}
      {isLoading && posts.length === 0 ? (
        <p style={{ color: "#999" }}>Loading posts...</p>
      ) : posts.length === 0 ? (
        <p style={{ color: "#999" }}>No posts yet. Create the first one above.</p>
      ) : (
        <ul style={{ listStyle: "none", padding: 0 }}>
          {posts.map((post) => (
            <li key={post.id} style={cardStyle}>
              <h3 style={{ margin: "0 0 4px" }}>{post.title}</h3>
              <p style={{ margin: "0 0 8px", color: "#444" }}>{post.body}</p>
              <span style={{ fontSize: 13, color: "#999" }}>
                {post.author} &middot;{" "}
                {new Date(post.createdAt).toLocaleString()}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const formStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 10,
  marginBottom: 32,
  padding: 20,
  border: "1px solid #eee",
  borderRadius: 12,
};

const inputStyle: React.CSSProperties = {
  padding: "10px 14px",
  fontSize: 15,
  borderRadius: 8,
  border: "1px solid #ddd",
  outline: "none",
};

const buttonStyle: React.CSSProperties = {
  padding: "10px 20px",
  fontSize: 15,
  borderRadius: 8,
  background: "#000",
  color: "#fff",
  border: "none",
  cursor: "pointer",
  alignSelf: "flex-start",
};

const cardStyle: React.CSSProperties = {
  padding: 20,
  borderBottom: "1px solid #eee",
};
