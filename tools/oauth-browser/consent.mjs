import assert from 'node:assert/strict';
import { randomBytes, createHash } from 'node:crypto';
import { chromium } from 'playwright';

const fixture = JSON.parse(process.env.OAUTH_BROWSER_FIXTURE);
const browser = await chromium.launch({
  ...(process.env.OAUTH_CHROMIUM_EXECUTABLE ? { executablePath: process.env.OAUTH_CHROMIUM_EXECUTABLE } : {}),
});
try {
  for (const client of fixture.clients) {
    const context = await browser.newContext();
    const discovery = await context.request.get(`${fixture.base}/.well-known/oauth-authorization-server`);
    assert.equal(discovery.status(), 200);
    const metadata = await discovery.json();
    assert.equal(metadata.issuer, fixture.base);
    assert.equal(metadata.authorization_response_iss_parameter_supported, true);
    assert.deepEqual(metadata.code_challenge_methods_supported, ['S256']);
    const page = await context.newPage();
    page.setDefaultTimeout(15000);
    // The fixture provides a local Discord stand-in on a different origin.
    // Browser redirects, cookies, Origin and Referer are never overridden.
    const verifier = randomBytes(32).toString('base64url');
    const state = randomBytes(24).toString('base64url');
    const params = new URLSearchParams({ response_type: 'code', client_id: client.id, redirect_uri: fixture.redirect, guild_id: fixture.guild, scope: 'vc.issue', state });
    await page.goto(`${fixture.base}/oauth2/authorize?${params}`);
    assert.equal(new URL(page.url()).searchParams.get('error_description'), 'code_challenge_required');
    assert.deepEqual(new URL(page.url()).searchParams.getAll('iss'), [metadata.issuer]);
    params.set('code_challenge', createHash('sha256').update(verifier).digest('base64url'));
    params.set('code_challenge_method', 'S256');
    await page.goto(`${fixture.base}/oauth2/authorize?${params}`);
    await page.locator('form[action="/oauth2/authorize"] button[type="submit"]').waitFor();
    const responsePromise = page.waitForResponse(r => r.url() === `${fixture.base}/oauth2/authorize` && r.request().method() === 'POST');
    await page.locator('button[type="submit"]').click();
    const approved = await responsePromise;
    const sent = await approved.request().allHeaders();
    // Before the fix Chromium sends Origin:null and the real endpoint returns 403.
    assert.equal(approved.status(), 303, `approval returned ${approved.status()}; Origin=${sent.origin}`);
    assert.equal(sent.origin, fixture.base);
    assert.equal(sent['sec-fetch-site'], 'same-origin');
    await page.waitForURL(url => url.origin === new URL(fixture.redirect).origin && url.pathname === '/client/callback');
    const returned = new URL(page.url());
    assert.equal(returned.searchParams.get('state'), state);
    assert.deepEqual(returned.searchParams.getAll('iss'), [metadata.issuer]);
    const code = returned.searchParams.get('code');
    assert.ok(code);
    const headers = client.method === 'client_secret_basic'
      ? { Authorization: `Basic ${Buffer.from(`${client.id}:browser-test-secret`).toString('base64')}` } : {};
    const form = { grant_type: 'authorization_code', client_id: client.id, redirect_uri: fixture.redirect, code };
    for (const invalid of [undefined, 'x'.repeat(43)]) {
      const response = await context.request.post(`${fixture.base}/oauth2/token`, { headers, form: { ...form, ...(invalid === undefined ? {} : { code_verifier: invalid }) } });
      assert.equal(response.status(), 400);
      assert.equal((await response.json()).error_description, 'invalid_code_verifier');
    }
    const exchanged = await context.request.post(`${fixture.base}/oauth2/token`, { headers, form: { ...form, code_verifier: verifier } });
    assert.equal(exchanged.status(), 200);
    const tokens = await exchanged.json();
    assert.ok(tokens.access_token);
    const refreshed = await context.request.post(`${fixture.base}/oauth2/token`, { headers, form: { grant_type: 'refresh_token', refresh_token: tokens.refresh_token } });
    assert.equal(refreshed.status(), 200);
    assert.notEqual((await refreshed.json()).refresh_token, tokens.refresh_token);
    // Recreate the old response policy deliberately: null origins remain refused.
    await page.goto(`${fixture.base}/oauth2/authorize?${params}`);
    const flowId = await page.locator('input[name="flow_id"]').inputValue();
    await page.goto(`${fixture.base}/test/null-origin?flow_id=${flowId}`);
    const refusedPromise = page.waitForResponse(r => r.url() === `${fixture.base}/oauth2/authorize` && r.request().method() === 'POST');
    await page.locator('button[type="submit"]').click();
    const refused = await refusedPromise;
    assert.equal((await refused.request().allHeaders()).origin, 'null');
    assert.equal(refused.status(), 403);
    assert.equal((await refused.json()).error_description, 'invalid_origin');
    console.log(`PASS ${client.method}: browser consent, PKCE, refresh, null-origin rejection`);
    await context.close();
  }
} finally {
  await browser.close();
}
