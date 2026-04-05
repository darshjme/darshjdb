"use server";

import { mutateServer } from "@darshan/nextjs/server";
import { revalidateTag } from "next/cache";

/**
 * Server Action: create a new post.
 *
 * Runs entirely on the server with admin privileges.
 * After inserting, revalidates the "posts" cache tag so ISR pages update.
 */
export async function createPost(title: string, body: string) {
  const result = await mutateServer(async (db) => {
    return db.collection("posts").insert({
      title,
      body,
      author: "Demo User",
      createdAt: Date.now(),
    });
  });

  // Trigger ISR revalidation for any page using the "posts" tag
  revalidateTag("posts");

  return result;
}

/**
 * Server Action: delete a post by ID.
 */
export async function deletePost(id: string) {
  await mutateServer(async (db) => {
    await db.collection("posts").delete(id);
  });

  revalidateTag("posts");
}
