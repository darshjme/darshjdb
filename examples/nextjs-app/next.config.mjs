/** @type {import('next').NextConfig} */
const nextConfig = {
  // Transpile workspace packages so Next.js can resolve them
  transpilePackages: ["@darshan/client", "@darshan/react", "@darshan/nextjs"],
};

export default nextConfig;
