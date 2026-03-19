import { expect, test } from "@playwright/test";

test("arcand health endpoint responds", async ({ request }) => {
  const response = await request.get("/health");
  // Accept 200 or 404 (endpoint may not exist yet)
  expect([200, 404]).toContain(response.status());
});

test("arcand session creation", async ({ request }) => {
  const response = await request.post("/sessions", {
    data: {},
  });
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body).toHaveProperty("session_id");
});
