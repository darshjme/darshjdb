import type { Metadata } from "next";
import { Providers } from "./components/providers";

export const metadata: Metadata = {
  title: "DarshanDB + Next.js",
  description: "Server Components + real-time client queries with DarshanDB",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body style={{ fontFamily: "system-ui, sans-serif", margin: 0 }}>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
