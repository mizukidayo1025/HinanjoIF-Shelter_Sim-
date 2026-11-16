# 分析レポート（印刷用ページ）

レポート画面の「↓ レポート保存」は、分析レポートを、画面の上に印刷用ページとして開きます。「印刷・PDFに保存」でブラウザーの印刷画面を開き、「PDFに保存」を選ぶとPDFとして保存できます（#470）。

レポートは、AIなしで作れます（#582）。条件・指標・推移・問題の継続・イベント・介入・B−Aの差は、中核が保存したRunから作るためです。Opusの考察は選択肢です。「レポート保存」の隣の「Opusの考察を付ける」を選んだときだけ付けます。この選択肢はAPIがONのときだけ選べます（OFFの間は、AIの機能に共通の表示で操作できません。[llm-integration.md](llm-integration.md)の「画面のON/OFF」）。

#470より前は、サーバーがReportLabでPDFを組版して返していました。ブラウザー版には組版の道具がないため、ローカル版・VM版も含めて、サーバーはレポートのデータを返し、画面が並べる形に揃えました。組み立ては画面の`demo-v2-print.js`の1か所だけです。

## 責務

- **中核**（`native/core/src/answers/pdf.rs`、#463）が作るもの：
  - レポートのデータ（条件、各BranchのKPI・時系列・問題の継続・記録したイベントと介入、B−Aの差）と、LLMが引用できる根拠（facts）。データと根拠は、各Branchの保存したRunから作ります。
  - 考察を求められたときだけ、Opusへの作業（`pdf_analysis`、`pdf-analysis-ja-v1`）と、答えの確かめ。確かめるのは、Opusであること、schema、各段落の根拠ID、数値・引用・因果断定の表現、仮説と提案の文末です。
  - 道の答え：`{"analysis", "data", "facts"}`の正準JSONです。考察なしは`"analysis": null`（`PdfPlan.report`）、考察つきは確かめた考察（`PdfAnalysis.report`）です。
- **ローカル版・VM版**（`web/pdf_report.py`）：各Branchのその分のRun・frame・KPIの表・journalを中核に渡します。考察を求められたときだけ、作業をRunのrouterに問い合わせ、答えを保存します。
- **ブラウザー版**：各Branchのその分のRun・frame・保存した標本・journalを中核に渡します（`native/wasm/js/browser_api.mjs`の`pdfReport`）。考察を求められたときは、ホームで「API を有効にする」を済ませた画面が要求に添える合言葉で、同じ作業をサイトの中継に頼みます（[llm-integration.md](llm-integration.md#ブラウザー版の中継419)）。答えはサーバーと同じbytesです。
- **画面**（`demo-v2-print.js`）：答えを、表紙・6章の文書に並べます（エグゼクティブサマリー、条件、時系列と問題の継続、介入と比較・重要イベント、AI分析と施策、限界と再現情報）。考察がないときは、最初の章に「AIの考察：なし」と書き、考察の節（AIの要旨・解釈・仮説・提案・追加検証、参照した根拠の表）と「AI分析と施策」の章を出しません。章の番号は詰めます（5章）。4つの指標の時系列はSVGで描き、欠測は補間しません。答えの文字列は、すべて文字として入れます（markupにしません）。施設表は先頭12件、介入表は先頭6件、重要イベント表は先頭10件を載せ、省略を明記します。実建物の形状を推測しないため、施設マップは載せません。
- **印刷**：`@media print`で、印刷用ページだけをA4で印刷します。章ごとに改ページし、表の見出しはページごとに繰り返します。ページの下に注意書きとページ番号を付けます（`@page`の余白の箱。対応しないブラウザーでは出ません）。

## 利用条件と対象

- **考察なし**：APIキーも合言葉も要りません。LLMには何も送りません（APIキーや合言葉があっても）。
- **考察つき**：APIキーのないサーバー、合言葉のない（または中継が合言葉を通さない）ブラウザー版は、409 `pdf_analysis_offline`で断ります。中継やAPIが失敗したとき（上限など）や、考察が確かめに通らないときも断ります。どれも、考察なしのレポートや代わりの文に黙って切り替えることはありません。
- **ローカル版・VM版の設定**（考察つき）：`SHELTERSIM_LLM_MODE=record`とAPI接続が要ります。`replay`では、一致する記録応答を使えます。失敗の扱いは`SHELTERSIM_LLM_ERROR_POLICY`に従います。執筆には`SHELTERSIM_OPUS_MODEL_ID`のOpusを使い、ほかのreview用途のモデル設定は変えません。
- **対象のBranchと時刻**：
  - 比較を取得しているときは、比較画面で選んでいる分岐とその親、比較の表示時刻を使います。
  - 取得していないときは、レポート画面の今のBranchと表示時刻を使います。
  - 単一Branchか、A/Bの2分岐です。
- **断るもの**：未計算の範囲、共通範囲の外、欠けたBranchは、作る前に断ります。途中の計算済み時刻では、途中結果と明記します。
- **閲覧のみのRun**：別のモデルの版で計算したRunは、保存した時点のほかは409 `pdf_saved_point_required`で断ります。

## API・保存・再現性

```text
POST /api/v1/projects/{project}/runs/{run}/pdf-report
{"branch_ids":["main","candidate"],"minute":120,"analysis":true}
```

`analysis`は、Opusの考察を付けるか（`true`か`false`）です。省くと`false`（考察なし）です。真偽値でなければ409 `pdf_invalid_analysis`です。省いたときに考察なしにするのは、明示して求めたときだけ外部のAPIに送るためです。

成功時は、中核の答え（`{"analysis", "data", "facts"}`のJSON。考察なしは`"analysis": null`）を返します。失敗時は、ほかの道と同じJSONの誤りです。GETの参照でProviderを呼ぶことはありません。

ローカル版・VM版は、考察つきの答えを`runs/{run}/pdf-reports/{hash}.json`に、schema_version=2の成果物として保存します。考察なしの答えは保存しません。求めるたびに保存したRunから作り直します（LLMを呼ばず、同じRunからは同じbytesになるためです）。そのため、考察なしの要求に保存した考察つきの答えを返すことも、その逆もありません。

- **中身**：入力のidentityと、答えのbytes（base64と、その検証hash）です。base64にするのは、保存の際に連絡先らしい文字列を伏せる処理で、答えのbytesを変えないためです。
- **identity**：Branchの順序・head（Branchの`checkpoint_hash`）、評価時刻、Scenario hash、Opusモデル、プロンプト版、レポートの版を含みます。同じidentityは、再起動の後も保存した答えをそのまま返します。
- **#470より前の成果物**：schema_version=1（PDFのbase64を持つもの）は保存がないものとして作り直します。Runの応答テープが同じ分析を答えるので、Opusは呼びません。
- **確かめに通らなかったとき**：再試行には別の試行番号を付けます（`runs/{run}/pdf-attempts/{hash}.json`）。失敗した利用記録は消しません。
- **ブラウザー版**：答えを保存しません。求めるたびに中継に頼みます（応答テープがないため、試行番号も要りません）。

書式・根拠の計算・確かめの契約を変えるときは、`pdf-report-ja-v1`と`pdf-analysis-ja-v1`の版を上げます。

## 検証

```bash
SHELTERSIM_LLM_MODE=offline pytest tests/integration/test_pdf_report.py tests/integration/test_demo_v2_routes.py
```

`tests/integration/test_pdf_report.py`は、偽のOpusで次を確かめます。

- 2つのBranch（介入あり）のレポートが中核のデータ・根拠と確かめた分析を答え、保存されること。同じレポートは問い合わせずに同じbytesで返すこと。
- 考察なしのレポートが、考察つきと同じデータ・根拠と`"analysis": null`を答え、Opusを呼ばず、保存もしないこと。APIキーがなくても同じbytesであること。APIキーがないときの考察つきは409 `pdf_analysis_offline`になること。
- schema_version=1の保存を作り直すこと。
- 確かめに通らない分析ではレポートを作らず、次の試行が別の依頼になること。
- ブラウザーの中継が、routerと同じ作業を送ること。

`native/wasm/tests/node_interventions.mjs`は、サーバーが偽のOpusで答えた2つの分岐のレポート（`native/core/tests/data/interventions_llm.json`）を、そのOpusの文を返す偽の中継で求めます。中継に送った作業がサーバーと同じであること、答えがサーバーと同じbytesであることを確かめます。考察なしのレポートは、合言葉があっても中継を呼ばず、サーバーと同じbytesであることを確かめます（`interventions.json`と`interventions_llm.json`）。

有料APIは使いません。
