import { queryServer } from "@darshjdb/nextjs/server";
import { PostList } from "./components/post-list";

/**
 * Home page -- Server Component.
 *
 * Fetches the initial post list on the server (fast, SEO-friendly),
 * then hands off to <PostList> which subscribes to real-time updates
 * on the client.
 */
export default async function HomePage() {
  // Server-side query -- runs at build time or on each request
  const initialPosts = await queryServer<Post>(
    {
      collection: "posts",
      orderBy: { createdAt: "desc" },
      limit: 20,
    },
    { revalidate: 10, tags: ["posts"] },
  );

  return (
    <main style={{ maxWidth: 640, margin: "40px auto", padding: "0 16px" }}>
      <h1>DarshJDB + Next.js</h1>
      <p style={{ color: "#666", marginBottom: 24 }}>
        Initial data loaded on the server. Real-time updates stream to the client.
      </p>
      <PostList initialPosts={initialPosts} />
    </main>
  );
}

/** Shape of a post document. */
interface Post {
  id: string;
  title: string;
  body: string;
  author: string;
  createdAt: number;
}
