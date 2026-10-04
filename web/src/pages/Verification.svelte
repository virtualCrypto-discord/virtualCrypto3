<script lang="ts">
  import { t } from "../i18n";
  // The old site's `readme.html.eex`, whose text the markup below is. It was a
  // server-rendered page there and is a client route here.
  //
  // It is worth a page of its own because of what the connect token is: this address
  // with an application in the query, which the operator pastes into a bot's own
  // description. So anything that gets somebody to put this address in a description is
  // doing it for a reason that is not theirs.
  //
  // The "11割" line is a parody of the warning Discord prints in its developer
  // console ("11/10 chance you're being scammed"); keep it when rewording.
</script>
<div class="columns">
  <div class="column is-7 is-offset-1-desktop">
    <div class="title">{t("verification.title")}</div>
    <div class="mt-5">
      <p>{t("verification.description")}</p>
      <p class="has-text-weight-bold is-size-5">
        {t("verification.scamWarning")}
      </p>
      <p class="has-text-weight-bold is-size-5 has-text-danger">
        {t("verification.connectionWarning")}
      </p>
    </div>
  </div>
</div>

<p><a href="/applications">{t("verification.applications")}</a></p>
