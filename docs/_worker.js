// Cloudflare Pages Advanced Mode worker — password-gate for zooni docs.
// Set the DOCS_PASSWORD environment variable in Cloudflare Pages settings.
// Visitors get a login page; a cookie keeps them authenticated for 7 days.

const COOKIE_NAME = "zooni_auth";
const MAX_AGE = 60 * 60 * 24 * 7;

function hashToken(password) {
  let hash = 0;
  const str = `zooni:${password}:salt_k9x2`;
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
  }
  return `z${(hash >>> 0).toString(36)}`;
}

function loginPage(error) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>Zooni Docs — Login</title>
<style>
  *{box-sizing:border-box;margin:0;padding:0}
  body{font-family:system-ui,sans-serif;background:#0d1117;color:#c9d1d9;
       display:flex;align-items:center;justify-content:center;min-height:100vh}
  .card{background:#161b22;border:1px solid #30363d;border-radius:12px;
        padding:2.5rem;width:100%;max-width:380px;text-align:center}
  h1{font-size:1.4rem;margin-bottom:.4rem}
  p.sub{color:#8b949e;font-size:.85rem;margin-bottom:1.5rem}
  input[type=password]{width:100%;padding:.65rem .9rem;border-radius:6px;
        border:1px solid #30363d;background:#0d1117;color:#c9d1d9;
        font-size:.95rem;margin-bottom:1rem;outline:none}
  input[type=password]:focus{border-color:#58a6ff}
  button{width:100%;padding:.65rem;border:none;border-radius:6px;
         background:#238636;color:#fff;font-size:.95rem;cursor:pointer;
         font-weight:600}
  button:hover{background:#2ea043}
  .err{color:#f85149;font-size:.85rem;margin-bottom:1rem}
</style>
</head>
<body>
<div class="card">
  <h1>Zooni Docs</h1>
  <p class="sub">Private documentation — enter password to continue</p>
  ${error ? `<p class="err">${error}</p>` : ""}
  <form method="POST">
    <input type="password" name="password" placeholder="Password" autofocus required/>
    <button type="submit">Enter</button>
  </form>
</div>
</body>
</html>`;
  return new Response(html, {
    status: error ? 401 : 200,
    headers: { "Content-Type": "text/html;charset=utf-8" },
  });
}

export default {
  async fetch(request, env) {
    const password = env.DOCS_PASSWORD;

    if (!password) {
      return env.ASSETS.fetch(request);
    }

    const cookies = request.headers.get("Cookie") || "";
    const expected = hashToken(password);
    if (cookies.includes(`${COOKIE_NAME}=${expected}`)) {
      return env.ASSETS.fetch(request);
    }

    if (request.method === "POST") {
      const form = await request.formData();
      const submitted = (form.get("password") || "").toString();

      if (submitted === password) {
        return new Response(null, {
          status: 302,
          headers: {
            Location: new URL(request.url).pathname,
            "Set-Cookie": `${COOKIE_NAME}=${expected}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=${MAX_AGE}`,
          },
        });
      }

      return loginPage("Incorrect password");
    }

    return loginPage();
  },
};
