// The landing page is static files (public/). This only runs for Party links,
// /party/<code>, which all show public/party.html; the page reads the code from
// the address. Phones with Tonearm open these links in the app instead
// (public/.well-known/assetlinks.json).
export default {
  async fetch(req: Request, env: { ASSETS: { fetch(req: Request): Promise<Response> } }): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname.startsWith("/party/")) {
      return env.ASSETS.fetch(new Request(new URL("/party", url), req));
    }
    return env.ASSETS.fetch(req);
  },
};
