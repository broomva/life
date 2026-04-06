import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // CesiumJS requires static asset serving for workers + WASM.
  // We copy Cesium assets to public/ via postinstall script.
  typescript: {
    ignoreBuildErrors: false,
  },
  // Allow opsisd backend for SSE.
  async rewrites() {
    return [
      {
        source: "/api/opsis/:path*",
        destination: `${process.env.OPSIS_URL ?? "http://localhost:3010"}/:path*`,
      },
    ];
  },
};

export default nextConfig;
