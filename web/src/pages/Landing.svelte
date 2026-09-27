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

<p><a href="/invite">Discord サーバーに導入する</a></p>
<p>導入後は、Discord で <code>/help</code> を入力すると使い方を確認できます。</p>

<!-- OAuth consent verifies identity with Discord for each request; there is no site login. -->

<!-- An ordinary link: it reloads, and the API answers it with this document.
     The documentation is the service's own, rendered from the same content the
     bot's `/help` answers with. -->
<p><a href="/document">使い方</a></p>

<style>
  h1 {
    margin-bottom: 0.5rem;
  }

</style>
