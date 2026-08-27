// Shared CORS handling for the Stripe/account edge functions.
//
// The desktop app and the static landing site both call these from a browser
// context (the Tauri webview, and drifterr.app). We allow the configured site
// origin plus localhost for development. Set SITE_URL to your production origin.

const SITE_URL = Deno.env.get("SITE_URL") ?? "https://drifterr.app";

const ALLOWED = new Set([
  SITE_URL,
  "https://drifterr.app",
  "http://localhost:3000",
  "http://localhost:8788", // Drifterr control API / menubar webview
  "http://127.0.0.1:8788",
  "tauri://localhost", // Tauri webview origin on macOS/Linux
  "https://tauri.localhost", // Tauri webview origin on Windows
]);

export function corsHeaders(origin: string | null): Record<string, string> {
  const allow = origin && ALLOWED.has(origin) ? origin : SITE_URL;
  return {
    "Access-Control-Allow-Origin": allow,
    "Access-Control-Allow-Headers":
      "authorization, x-client-info, apikey, content-type",
    "Access-Control-Allow-Methods": "POST, GET, OPTIONS",
    "Vary": "Origin",
  };
}

export function preflight(req: Request): Response | null {
  if (req.method === "OPTIONS") {
    return new Response("ok", {
      headers: corsHeaders(req.headers.get("origin")),
    });
  }
  return null;
}

export function json(
  body: unknown,
  status: number,
  origin: string | null,
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...corsHeaders(origin), "Content-Type": "application/json" },
  });
}
