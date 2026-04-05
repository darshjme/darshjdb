"use client";

import { DarshanProvider } from "@darshan/nextjs";

/**
 * Client-side providers wrapper.
 *
 * DarshanProvider reads NEXT_PUBLIC_DARSHAN_URL from the environment
 * automatically. Wrap the entire app so every page can use real-time hooks.
 */
export function Providers({ children }: { children: React.ReactNode }) {
  return (
    <DarshanProvider url={process.env.NEXT_PUBLIC_DARSHAN_URL}>
      {children}
    </DarshanProvider>
  );
}
