// Serverless redirect so downloads stay on our domain — the visitor hits
// drifterr.app/download/<os>, we resolve the latest release's installer for that
// OS and 302 straight to the binary. The browser downloads the file; nobody ever
// lands on the GitHub repo page.
//
// Runs on Vercel (Node serverless). vercel.json rewrites /download/<os> here.

const REPO = "Lounes-M/drifterr";

// Pick the right asset for an OS from a release's asset list. Order = preference:
// the first matcher that hits wins (universal > Apple silicon > Intel, etc.).
const MATCHERS = {
  mac: [/universal.*\.dmg$/i, /(aarch64|arm64).*\.dmg$/i, /\.dmg$/i],
  win: [/setup\.exe$/i, /\.msi$/i, /\.exe$/i],
  linux: [/\.appimage$/i, /\.deb$/i],
};

function normalizeOS(raw) {
  const s = String(raw || "").toLowerCase();
  if (/mac|osx|darwin|apple/.test(s)) return "mac";
  if (/win/.test(s)) return "win";
  if (/linux|appimage|deb/.test(s)) return "linux";
  return null;
}

function pickAsset(assets, os) {
  for (const re of MATCHERS[os] || []) {
    const hit = assets.find((a) => re.test(a.name));
    if (hit) return hit;
  }
  return null;
}

export default async function handler(req, res) {
  // /download/<os> is rewritten to /api/download?os=<os>; also accept ?os= directly.
  const os = normalizeOS(req.query?.os ?? req.query?.path);

  if (!os) {
    // Unknown / unspecified OS → bounce back to the page's download section so the
    // visitor can choose, instead of guessing wrong.
    res.statusCode = 302;
    res.setHeader("Location", "/#download");
    return res.end();
  }

  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "drifterr-landing",
        ...(process.env.GITHUB_TOKEN ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` } : {}),
      },
    });

    if (!r.ok) {
      // No published release yet (404) or API hiccup — don't dump people on GitHub.
      res.statusCode = 503;
      res.setHeader("content-type", "text/html; charset=utf-8");
      return res.end(
        "<!doctype html><meta charset=utf-8><title>Coming soon</title>" +
          "<body style=\"font:16px/1.6 system-ui;max-width:32rem;margin:18vh auto;padding:0 1.5rem;text-align:center\">" +
          "<h1>Almost there</h1><p>The installer isn’t published yet. Check back shortly.</p>" +
          "<p><a href=\"/\">← Back to drifterr</a></p>"
      );
    }

    const release = await r.json();
    const asset = pickAsset(release.assets || [], os);

    if (!asset) {
      res.statusCode = 404;
      res.setHeader("content-type", "text/html; charset=utf-8");
      return res.end(
        "<!doctype html><meta charset=utf-8><title>No build</title>" +
          "<body style=\"font:16px/1.6 system-ui;max-width:32rem;margin:18vh auto;padding:0 1.5rem;text-align:center\">" +
          `<h1>No ${os} build</h1><p>This release has no installer for your platform yet.</p>` +
          "<p><a href=\"/\">← Back to drifterr</a></p>"
      );
    }

    // Short cache so a freshly cut release shows up quickly but we don't hammer the API.
    res.setHeader("Cache-Control", "public, max-age=300, s-maxage=300");
    res.statusCode = 302;
    res.setHeader("Location", asset.browser_download_url);
    return res.end();
  } catch {
    res.statusCode = 502;
    res.setHeader("content-type", "text/plain; charset=utf-8");
    return res.end("Download temporarily unavailable. Please try again.");
  }
}
