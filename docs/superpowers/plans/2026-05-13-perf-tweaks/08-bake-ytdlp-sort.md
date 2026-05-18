# Task 8 — Bake T#4: yt-dlp `-S` sort fallback against `news_orgs` fixture

**Goal:** Empirically verify that T7's `-S +size,+br,+res,+fps` flag (a) does not regress the `download`-format success path and (b) prefers smaller formats when the `b[vcodec=h264]/b` fallback runs. Per the spec: this change is likely defensive (T13 reported 100% selector hit rate; fallback never ran). Honestly document the inert-on-this-fixture case if that's what we observe.

**Spec commit:** 7 — `bake(fetcher): T#4 yt-dlp -S sort fallback bake notes`.

**ADRs directly relevant:**
- **AD0017** — operational "done" contract for batch validation (the `news_orgs` 20-video fixture is the ratified bake set).

**Files:**
- Append: `docs/SRC-BAKE-NOTES.md` (a new dated section for this bake)
- Possibly append: `docs/bake-findings.md` (if the change is inert against the current fixture, note it as an operational observation)
- Possibly revert: T7 commit (commit 6 in the spec sequence) if the bake reveals a regression on the success path

**Where this task runs:** the SRC A10 workspace (or wherever `news_orgs` fixture URLs are reachable + yt-dlp pinned version is installed). The dev machine works too if it has yt-dlp; this bake doesn't require GPU.

---

- [ ] **Step 1: Locate the `news_orgs` fixture**

Run, from the worktree:
```bash
find . -type f \( -name "news_orgs*" -o -name "*news_orgs*" \) 2>/dev/null | head -10
grep -rn "news_orgs" docs/ scripts/ tests/ 2>/dev/null | head -10
```

Expected: locate the fixture file (likely a list of TikTok URLs or video IDs, possibly a CSV or JSON). If the fixture is not in the repo, ask the operator for its location — T13's bake used it; the operator knows.

Record the fixture path and the count of URLs (should be 20 per T13).

- [ ] **Step 2: Pre-change baseline — capture per-URL format selection**

Check out the commit immediately BEFORE T7 (i.e., the AD0021 commit from T6, which is commit 5 in the spec sequence):
```bash
git log --oneline | head -10
# Note the SHA of the commit before T7 (the "docs(adr): add AD0021..." commit).
git checkout <SHA_BEFORE_T7>
```

For each URL in the `news_orgs` fixture, run yt-dlp with the pre-T7 args (which already include T3's explicit ffmpeg flags but lack T7's `-S` sort) and capture which format ID won:

```bash
mkdir -p /tmp/bake-t8-pre
cd /tmp/bake-t8-pre
while IFS= read -r url; do
  echo "=== $url ==="
  yt-dlp \
    --no-playlist --no-warnings --quiet \
    -f 'download/b[vcodec=h264]/b' \
    -x --audio-format wav \
    --postprocessor-args 'ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1' \
    --print '%(id)s\t%(format_id)s\t%(filesize_approx)s\t%(width)sx%(height)s' \
    "$url" 2>&1 | tee -a pre-bake.log
done < <PATH_TO_NEWS_ORGS_URL_LIST>
```

(Replace `<PATH_TO_NEWS_ORGS_URL_LIST>` with the fixture path from Step 1. The `--print` flag emits a single line per URL without actually downloading; this captures the selector outcome cheaply. If the fixture is JSON, use `jq` to extract URLs first.)

Expected: 20 lines, one per URL, each showing `<video_id>\t<format_id>\t<filesize>\t<resolution>`. Per T13, all 20 should show `format_id = download`.

- [ ] **Step 3: Switch to T7 and run the same capture**

```bash
cd <BACK TO WORKTREE>
git checkout feat/perf-tweaks
# Verify T7's commit is HEAD (or HEAD~ depending on how many commits beyond T7 you've made):
git log --oneline | head -5
```

Run the same loop with the post-T7 args (the only change is the addition of `-S +size,+br,+res,+fps`):

```bash
mkdir -p /tmp/bake-t8-post
cd /tmp/bake-t8-post
while IFS= read -r url; do
  echo "=== $url ==="
  yt-dlp \
    --no-playlist --no-warnings --quiet \
    -f 'download/b[vcodec=h264]/b' \
    -S '+size,+br,+res,+fps' \
    -x --audio-format wav \
    --postprocessor-args 'ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1' \
    --print '%(id)s\t%(format_id)s\t%(filesize_approx)s\t%(width)sx%(height)s' \
    "$url" 2>&1 | tee -a post-bake.log
done < <PATH_TO_NEWS_ORGS_URL_LIST>
```

- [ ] **Step 4: Compare pre vs post**

Run:
```bash
diff <(sort /tmp/bake-t8-pre/pre-bake.log | grep -v "^===") \
     <(sort /tmp/bake-t8-post/post-bake.log | grep -v "^===")
```

Three possible outcomes:

**(a) No diff — change is inert on this fixture.** Expected per T13's 100% selector hit rate. Proceed to Step 5a.

**(b) Diff shows post-change selecting different (smaller) formats for some videos.** The fallback ran on some URLs and `-S` reordered the result. Proceed to Step 5b.

**(c) Diff shows post-change FAILING on some videos that worked pre-change.** Regression on the success path. Proceed to Step 5c (revert).

- [ ] **Step 5a (inert outcome): Append to `docs/bake-findings.md`**

If outcome (a) — the most likely case — append to `docs/bake-findings.md`:

```markdown
---

## yt-dlp `-S +size,+br,+res,+fps` is inert against `news_orgs` fixture

**Found in:** Perf-tweaks T8 bake (commits <T7_SHA> + <T8_SHA>).
**Disposition:** Change is correct and defensive; bake confirms no regression on the success path. No follow-up needed unless a future fixture exercises the `b[vcodec=h264]/b` fallback.
**Trigger to revisit:** if a future bake or production run observes the fallback running and we want to confirm `-S` is actually preferring smaller formats.

**Observation:** All 20 news_orgs URLs resolved to `format_id = download` both pre- and post-T7 (matching T13's 100% selector hit rate). `-S` only orders within a selector match; since the first selector token `download` is a literal format ID, the success path bypasses sort. The change therefore had no measurable effect on this fixture.

**Why we kept the change:** defensive — TikTok's `download` format depends on creator settings (some videos disable downloads) and on yt-dlp's `download`-format extractor staying functional (yt-dlp #15891 and #16622 document upstream non-determinism in adjacent paths). When the fallback runs, `-S` ensures we don't accidentally pick a needlessly-large h264 variant.
```

- [ ] **Step 5b (diff with smaller formats — measured win): Append to `docs/SRC-BAKE-NOTES.md`**

Append:

```markdown
---

## Perf-tweaks T8 bake: yt-dlp `-S` sort fallback

**Date:** <YYYY-MM-DD>
**Branch:** `feat/perf-tweaks` @ <T7_SHA>
**Fixture:** news_orgs (20 URLs)

**Outcome:** measured win on N videos where fallback ran.

| Video ID | Pre-change format | Post-change format | Pre size (bytes) | Post size (bytes) | Delta |
|----------|-------------------|--------------------|-------------------|--------------------|-------|
| <vid1>   | <fmt1>            | <fmt2>             | <N>               | <M>                | <-X>  |
| ...      | ...               | ...                | ...               | ...                | ...   |

Total bytes saved across N affected videos: <SUM>.

The `-S` sort prefers smaller-size formats when the `b[vcodec=h264]/b` fallback runs. M of 20 videos exercised the fallback; `-S` reduced the average format size by <Y>%.
```

- [ ] **Step 5c (regression): Revert T7**

```bash
git revert <T7_SHA>
# Or, if T7 is the most recent commit: git revert HEAD
```

The revert commit message must include:

```
revert: bake gate failed for T#4 yt-dlp -S sort fallback

Bake against news_orgs revealed <N>/20 URLs that worked
pre-change failed post-change with <ERROR_DESCRIPTION>. Likely
cause: <hypothesis>. -S syntax may not match the pinned yt-dlp
version's expectations, OR the fallback selector resolution
changed shape.

Pre-change selector outcomes:
<paste relevant lines from /tmp/bake-t8-pre/pre-bake.log>

Post-change failures:
<paste relevant lines from /tmp/bake-t8-post/post-bake.log>

Adds a docs/bake-findings.md entry for future re-investigation.
```

Also append a `docs/bake-findings.md` entry with `**Hypothesis (unverified):**` prefixing any guess about the cause (per AD0020).

- [ ] **Step 6: Stage the bake notes and commit**

For outcomes (a) and (b):
```bash
git add docs/bake-findings.md  # outcome (a)
# OR
git add docs/SRC-BAKE-NOTES.md  # outcome (b)
git commit -m "$(cat <<'EOF'
bake(fetcher): T#4 yt-dlp -S sort fallback bake notes

Empirically confirmed against the news_orgs 20-URL fixture.

<Either: "Outcome inert on this fixture; documented as defensive
per spec § Bake plan #4 honest-reporting clause." OR: "Outcome:
measured byte savings on N/20 videos where the fallback ran; see
SRC-BAKE-NOTES.md for per-video table.">

yt-dlp version: <FROM Step 1 of T7>
Selector hit rate: <N>/20 download / <M>/20 fallback
Total bytes saved (post-pre): <S> bytes

Refs: AD0017

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

For outcome (c), the revert commit (Step 5c) is the only commit in this task; no separate bake-notes commit.

---

## Self-check

- [ ] Pre-change and post-change `yt-dlp --print` captures exist (in `/tmp/bake-t8-*/`); their diff is in the commit body or in the appended bake notes.
- [ ] One of `docs/bake-findings.md` (outcome a/c) or `docs/SRC-BAKE-NOTES.md` (outcome b) gained a new dated section.
- [ ] If outcome (c), the revert commit lands AND a `docs/bake-findings.md` entry routes the finding.
- [ ] The bake notes are honest about whether this is a measured win or a defensive change.
