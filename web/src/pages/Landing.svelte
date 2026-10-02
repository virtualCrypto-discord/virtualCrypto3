<script lang="ts">
  import Message from "../Message.svelte";
  import { t } from "../i18n";
</script>

<!--
  The prose is deliberately thin. The old site's landing copy has not been read
  here, and inventing a description of the service is the kind of thing that is
  never wrong enough to be caught — so this says only what the documents say.
-->
<h1>VirtualCrypto</h1>
<p>{t("landing.description")}</p>

<p><a href="/invite">{t("landing.invite")}</a></p>
{#snippet command()}<code>/help</code>{/snippet}
<p><Message id="landing.help" slots={{ command }} /></p>

<!-- OAuth consent verifies identity with Discord for each request; there is no site login. -->

<!-- An ordinary link: it reloads, and the API answers it with this document.
     The documentation is the service's own, rendered from the same content the
     bot's `/help` answers with. -->
<p><a href="/document">{t("landing.documentation")}</a></p>

<style>
  h1 {
    margin-bottom: 0.5rem;
  }

</style>
