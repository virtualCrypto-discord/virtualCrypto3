<script lang="ts">
  import { loginUrl } from "../api";

  // Where to come back to. The old site resumed the request that failed, which a
  // SPA cannot see; what it can see is the page it is on, and that is the honest
  // answer to "continue".
  const here = window.location.pathname + window.location.search;
  const login = loginUrl(here === "/" ? undefined : here);
</script>

<!--
  The prose is deliberately thin. The old site's landing copy has not been read
  here, and inventing a description of the service is the kind of thing that is
  never wrong enough to be caught — so this says only what the documents say.
-->
<h1>VirtualCrypto</h1>
<p>Discord のサーバーで使う通貨を扱うためのサービスです。</p>

<p><a class="login" href={login}>Discord でログイン</a></p>

<!-- An ordinary link: it reloads, and the API answers it with this document. -->
<p><a href="/me">アカウント</a></p>

<style>
  h1 {
    margin-bottom: 0.5rem;
  }

  .login {
    display: inline-block;
    padding: 0.5rem 1rem;
    border-radius: 0.5rem;
    background: #5865f2;
    color: white;
    text-decoration: none;
    font-weight: 600;
  }
</style>
