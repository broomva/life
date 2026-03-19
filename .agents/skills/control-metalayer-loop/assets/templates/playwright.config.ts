import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests/e2e/web",
  timeout: 45_000,
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL || process.env.APP_BASE_URL || "http://127.0.0.1:3000",
    trace: "retain-on-failure",
  },
  reporter: [["line"]],
});
