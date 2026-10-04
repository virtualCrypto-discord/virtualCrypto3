<script lang="ts">
  import Message from "../Message.svelte";
  import { t, type MessageKey } from "../i18n";

  // Every claim below is one the guide already makes (`/document/start`), so the
  // landing page never says more about the service than the documents do. The
  // command names are protocol identifiers and stay out of the catalog.
  const features: { commands: string[]; title: MessageKey; body: MessageKey }[] = [
    { commands: ["/create"], title: "landing.feature.create.title", body: "landing.feature.create.body" },
    { commands: ["/pay", "/claim"], title: "landing.feature.pay.title", body: "landing.feature.pay.body" },
    { commands: ["/bal", "/info"], title: "landing.feature.balance.title", body: "landing.feature.balance.body" },
    { commands: ["/issue"], title: "landing.feature.issue.title", body: "landing.feature.issue.body" },
  ];
</script>

<div class="landing">
<header class="hero">
  <img class="logo" src="/static/images/logo.jpg" alt="" width="128" height="128" />
  <h1>VirtualCrypto</h1>
  <p class="lead">{t("landing.description")}</p>
  <p class="note">{t("landing.note")}</p>
  <div class="actions">
    <a class="button primary" href="/invite">{t("landing.invite")}</a>
    <!-- An ordinary link: it reloads, and the API answers it with this document. -->
    <a class="button" href="/document/start">{t("landing.documentation")}</a>
  </div>
</header>

<section>
  <h2>{t("landing.featuresHeading")}</h2>
  <ul class="features">
    {#each features as feature}
      <li>
        <p class="commands">
          {#each feature.commands as command}<code>{command}</code>{/each}
        </p>
        <h3>{t(feature.title)}</h3>
        <p>{t(feature.body)}</p>
      </li>
    {/each}
  </ul>
  <p class="aside">
    {#snippet applications()}<a href="/document/applications">{t("landing.applicationsLink")}</a>{/snippet}
    <Message id="landing.applications" slots={{ applications }} />
  </p>
</section>

<section>
  <h2>{t("landing.startHeading")}</h2>
  <ol class="steps">
    <li>
      {#snippet invite()}<a href="/invite">{t("landing.inviteLink")}</a>{/snippet}
      <Message id="landing.step.invite" slots={{ invite }} />
    </li>
    <li>
      {#snippet create()}<code>/create</code>{/snippet}
      <Message id="landing.step.create" slots={{ create }} />
    </li>
    <li>
      {#snippet pay()}<code>/pay</code>{/snippet}
      {#snippet issue()}<code>/issue</code>{/snippet}
      <Message id="landing.step.distribute" slots={{ pay, issue }} />
    </li>
  </ol>
  {#snippet command()}<code>/help</code>{/snippet}
  <p class="aside"><Message id="landing.help" slots={{ command }} /></p>
</section>

<!-- OAuth consent verifies identity with Discord for each request; there is no site login. -->

<footer>
  <a href="/document">{t("landing.footer.documentation")}</a>
  <a href="/document/faq">{t("landing.footer.faq")}</a>
  <a href="/document/applications">{t("landing.footer.applications")}</a>
  <a href="/support">{t("landing.footer.support")}</a>
</footer>
</div>

<style>
  /* The purple is the logo's outline; the page otherwise stays quiet. */
  .landing {
    --accent: #5b21d6;
    --accent-strong: #4a17b3;
    --muted: #5f6368;
    --line: #e6e4ec;
    --line-strong: #cfcbdb;
    --surface: #faf9fc;
    line-height: 1.7;
  }

  .hero {
    text-align: center;
    padding: 1rem 0 2.5rem;
    border-bottom: 1px solid var(--line);
  }

  .logo {
    display: block;
    margin: 0 auto 1rem;
    width: 128px;
    height: 128px;
    border-radius: 50%;
  }

  h1 {
    margin: 0 0 0.75rem;
    font-size: 2.25rem;
    letter-spacing: 0.01em;
  }

  .lead {
    margin: 0;
    font-size: 1.15rem;
  }

  .note {
    margin: 0.5rem 0 0;
    color: var(--muted);
    font-size: 0.95rem;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.75rem;
    margin-top: 1.75rem;
  }

  .button {
    display: inline-block;
    padding: 0.6rem 1.25rem;
    border: 1px solid var(--line-strong);
    border-radius: 0.5rem;
    color: inherit;
    font-weight: 600;
    text-decoration: none;
  }

  .button:hover {
    background: var(--surface);
  }

  .button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }

  .button.primary:hover {
    background: var(--accent-strong);
  }

  section {
    padding: 2.5rem 0 0;
  }

  h2 {
    margin: 0 0 1.25rem;
    font-size: 1.25rem;
  }

  .features {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
    gap: 1rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .features li {
    padding: 1rem 1.25rem;
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    background: var(--surface);
  }

  .features h3 {
    margin: 0.4rem 0 0.35rem;
    font-size: 1rem;
  }

  .features p {
    margin: 0;
    font-size: 0.95rem;
    line-height: 1.7;
  }

  .commands {
    display: flex;
    gap: 0.4rem;
  }

  .commands code {
    color: var(--accent);
    font-weight: 600;
  }

  .steps {
    margin: 0;
    padding-left: 1.5rem;
    line-height: 2;
  }

  .aside {
    margin: 1rem 0 0;
    color: var(--muted);
    font-size: 0.95rem;
  }

  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.9em;
  }

  footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.5rem 1.5rem;
    margin-top: 3.5rem;
    padding-top: 1.5rem;
    border-top: 1px solid var(--line);
    font-size: 0.9rem;
  }

  footer a {
    color: var(--muted);
  }

  a {
    color: var(--accent);
  }
</style>
