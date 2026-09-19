<script lang="ts">
  // Nothing to do here: the page is prose. The block is what makes the file a
  // module, which is what the shell's import of it resolves against.
</script>

<!--
  The prose is deliberately thin. The old site's landing copy has not been read
  here, and inventing a description of the service is the kind of thing that is
  never wrong enough to be caught — so this says only what the documents say.
-->
<h1>VirtualCrypto</h1>
<p>Discord のサーバーで使う通貨を扱うためのサービスです。</p>

<!-- No login here on purpose: the only flow that needs a session is OAuth2's,
     and it sends the browser to `/login` itself when the consent screen is
     reached without one. A button here would be a second way in for nobody. -->

<!-- An ordinary link: it reloads, and the API answers it with this document. -->

<style>
  h1 {
    margin-bottom: 0.5rem;
  }

</style>
