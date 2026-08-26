// Talking to the local Drifterr control API.
//
// Shared by the service worker (via `importScripts`) and the popup (via a
// `<script>` tag), so there is exactly one definition of the base URL, the
// pairing token and the 401 path. Both are classic scripts, so this file defines
// globals rather than exporting — matching the rest of the extension.
//
// # Why there is a token at all
//
// The control API used to be open to any caller on the machine, which meant any
// *website* the user had open could read their sessions out of it. It is now
// authenticated. The extension is not launched by Drifterr and cannot read the
// app's data directory, so it is the one consumer that has to be paired by hand:
// the user copies the token from the panel once and it is stored here.
//
// The token is stored in `chrome.storage.local` rather than `sync`: it is
// specific to this machine's Drifterr install, and syncing it to the user's other
// browsers would pair them against a token that is not theirs.

/* eslint-env webextensions */

const DRIFTERR_BASE = "http://localhost:8788";
const DRIFTERR_TOKEN_KEY = "drifterrToken";

/** Read the stored pairing token, or "" when the extension has not been paired. */
async function drifterrToken() {
  try {
    const got = await chrome.storage.local.get(DRIFTERR_TOKEN_KEY);
    const t = got && got[DRIFTERR_TOKEN_KEY];
    return typeof t === "string" ? t.trim() : "";
  } catch (_e) {
    return "";
  }
}

/** Store a pairing token. An empty value clears it. */
async function drifterrSetToken(token) {
  const clean = typeof token === "string" ? token.trim() : "";
  await chrome.storage.local.set({ [DRIFTERR_TOKEN_KEY]: clean });
  return clean;
}

/**
 * Fetch a control-API path with the pairing token attached.
 *
 * Throws `DrifterrUnpaired` on 401 rather than returning it, so no caller can
 * mistake "you are not paired" for "there is no drift". That distinction is the
 * whole point: a monitoring tool that silently reports nothing because it is
 * misconfigured is worse than one that says it is misconfigured.
 */
async function drifterrFetch(path, init) {
  const token = await drifterrToken();
  const base = init || {};
  const headers = { ...(base.headers || {}) };
  if (token) headers["X-Drifterr-Token"] = token;
  const res = await fetch(DRIFTERR_BASE + path, { ...base, headers });
  if (res.status === 401) {
    const err = new Error("drifterr: not paired");
    err.name = "DrifterrUnpaired";
    throw err;
  }
  return res;
}

/** True when the failure was specifically "no valid token". */
function drifterrIsUnpaired(err) {
  return !!err && err.name === "DrifterrUnpaired";
}

// Reachable from the tests and from Node, without disturbing the browser globals.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { drifterrFetch, drifterrToken, drifterrSetToken, drifterrIsUnpaired };
}
