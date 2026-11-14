# 避難所IF！ の画面（`/`）

利用者向けの唯一の画面で、HTML/CSS/JavaScriptの6画面（ホーム、シナリオ設定、シミュレーション、分岐比較、レポート・対話、詳細）です（以前の画面`/demo`、`/admin`は#376でなくし、その道は`/demo-v2`へ転送します）。画面の道は`/`です。前の道の`/demo-v2`は`/`へ転送します（ブラウザー版では同じ頁を出し、アドレスを`/`に替えます）。ファイル名の`demo-v2*`は中の名前として変えません（#376）。クエリのない`/`とロゴは、ホームを開きます（#371）。ヘッダーの時刻・再生速度・再生ボタンは、時刻のある画面（シミュレーション・分岐比較）だけに出します。サイドバーの下は、開いているシナリオと注意書き1行だけです。シナリオ設定は、シナリオを開いているときはその条件（変えられません）と「新しいシナリオを作る」を、開いていないときは新しいシナリオの入力を出します。地域の避難所の一覧を入れずに作ったブラウザー版のサイトでは、新しいシナリオを作れないので、シナリオを開くまでシナリオ設定を押せず、ホームの「新しいシナリオを作る」も押せません（#372）。「見る→分岐を作る→比べる」は分岐比較の画面の中でできます。介入の欄は分岐比較の画面に置き、シミュレーション画面には、表示中の時刻から分岐を作る「この時刻から分岐を作る →」だけを置きます。分岐比較の画面は、比べられる分岐があれば開いたときに読み込み、まだないときは開いている分岐をAに出し、Bの場所に分岐を作るとそこに並ぶことを出します。計算の欄は、開いている分岐に未計算の時刻があるときだけ出します。再生のボタンはヘッダーの1つだけです（#374）。指標は、シミュレーション画面の6枚のカードと、レポートの1行の帯（同じ定義・同じ時刻の記録の値）で出します。レポートは推移のグラフと要約を主にし、シミュレーション画面と同じになる「参照時点の避難所マップ」は置きません。結果への質問の欄は、APIキーのない環境（ブラウザー版・offline）では、質問にAIの回答を作れないこと（決まった答えを返すこと）を出します（#375）。WorldState、Run、Checkpoint、EffectProgram、モデル係数の契約を変更しません。

2026-10-25の参照画像との再比較、修正内容、両解像度での実測、残る相違は [視覚レビュー記録](ui-backup-visual-review.md) にまとめています。

最新main統合後の模式図・個別キャラクター・連続再生は [マップとアニメーションの実装記録](ui-backup-map-motion.md) を参照してください。以前の集合表示主体のレビューは改修前の記録です。

その後の4方向スプライト、通路を通る実移動の可視化、正確な再生倍率、録画・人数計測は [ゲーム品質改善の記録](ui-backup-gameplay.md) が最新仕様です。

## 起動

バックアップworktree内に専用venvを用意します。既存ディレクトリのvenvへeditable installして、既存作業の参照先を変更しないでください。デモはClaude APIへ接続する`record` modeで起動し、APIエラー時にoffline fallbackせず理由を表示します。API keyはGit管理外の環境変数またはローカルの`.env`から読み込ませてください（アプリは`.env`を自動読込しません）。次の`pip install`のあと、起動の前に、Rustのkernelを同じvenvへ導入します（[開発環境](development.md#rustのkernel)）。

```sh
python3.12 -m venv .venv
.venv/bin/python -m pip install -e '.[dev,llm]'
# keyを保管したローカル.envを使う場合。パスは手元の保存先に合わせます。
set -a
. /path/to/local/.env
set +a
test -n "${ANTHROPIC_API_KEY:-}" || { echo 'ANTHROPIC_API_KEY is required' >&2; exit 1; }
SHELTERSIM_LLM_MODE=record SHELTERSIM_LLM_ERROR_POLICY=stop \
  .venv/bin/python -m sheltersim web \
  --runs-dir /tmp/hinanjo-if-backup-runs --host 127.0.0.1 --port 8876
```

[http://127.0.0.1:8876/](http://127.0.0.1:8876/) を開きます。生成Runは専用ローカル保存先へ保存されます。実際に計算したProject Runの記録を再生します。再生倍率は実時間との比率で、既定の60×は実時間1秒でシミュレーション1分です。計算速度の指定ではありません。未計算時刻へはシークできず、記録の読込待ちは時刻を保持します。

## 画面の用語（#373）

画面に出す文（ボタン、見出し、お知らせ、誤りの文、`title`・`aria-label`）は、次の語にそろえます。コードの識別子、APIのJSON、設計文書の語（Run、Branch、Checkpoint）は変えません。`tests/unit/web/test_static_demo_v2.py`が、画面の文にRun・Branch・世界線・Checkpointがないことを確かめます。

| 内部の語 | 画面の語 |
|---|---|
| Run（承認して作った計算の単位） | シナリオ |
| Branch、世界線 | 分岐 |
| `main`の分岐 | 元のシナリオ |
| Checkpoint | 保存した時点 |
| KPI | 指標 |
| tick | 各分の計算 |
| fallback | 代替処理 |
| Zone | 区画 |
| EffectProgramの`support_mode` | 再現の方法（「モデルでそのまま再現」など） |

同じ条件のRunが複数あるとき（作り直し、seed違い）も、それぞれを1つのシナリオと呼びます。名前（自治体・災害・人数・日数）が同じシナリオは、名前の後にseedを、seedも同じならRunのIDの末尾4文字を付けて見分けます。

## 画面とAPIの対応

`P=/api/v1/projects/{project}`、`R=P/runs/{run}`、`B=R/branches/{branch}`。

| 画面・機能 | 既存API | 調査時の状態 / 新UIでの対応 |
|---|---|---|
| ホーム | GET /api/v1/projects、GET P | 計算済みの例（Project `sample`の計算を終えたRun）、ブラウザー版で計算するRunと「計算をやり直す」、自分のシナリオ（このブラウザーの自分のProject）を並べる。読むだけで何も変えない（#371） |
| Project作成・保存Run復元 | POST /api/v1/projects、GET P、GET P/runs | 実装済み / 新しいシナリオは例のProjectでなく自分のProjectに作る。開いているRunはURLが持つ |
| 自然言語・基本条件・任意資源 | POST P/scenario-preview | 実装済み / 入力値を優先し、出典・補完仮定・警告・ブロッカーを表示 |
| 施設種別 | request.facility_conditions | 条件の記録に対応 / 実施設の図面・適合性や別形状を推測しない |
| 人口構成・施設概要 | preview.scenario、facility_design | 実装済み / 記録年齢の構成図と生成Geometryを描画 |
| 明示承認・Run作成 | POST P/scenario-approve | 実装済み / preview_id、approved_hash、approved:trueを送信 |
| 計算・停止・進捗 | POST R/resume、R/cancel、GET R | 実装済み / 計算と再生を独立、保存済み時刻を進捗表示。計算中はGET Rで状態だけを確認し、表示中のRun・Branch・時刻とhead checkpoint_hashが変わったときだけtimelineを再取得 |
| マップ・人物・スタッフ・正式KPI | GET B/timeline?minute=M | 実装済み / 明示された模式図と実座標の切替、欠測位置の行動を補わない |
| レイヤー・拡大縮小・人物選択 | timelineデータによるブラウザー描画 | 実装済み / 独立Canvas描画、ドラッグ・キー移動・集合内の人物選択。健康レイヤーは記録された疲労・空腹・水分ニーズで、医学診断ではない |
| 再生・停止・速度・巻戻し | GET R/replay-frames | 実装済み / 1分刻みの記録を最大8境界ずつ取得。1/5/15/60/120/240×の実時間時計で連続描画し、通信待ちは時刻を保持 |
| 自然言語介入プレビュー | POST B/intervention-preview | 実装済み / 変換作用・検証結果・support_mode・制限を表示 |
| 介入の明示承認・独立分岐 | POST B/intervention-apply | 実装済み / hashと入力・時刻・分岐を照合。変更で再承認が必要 |
| 同期比較 | GET R/compare?branch_ids=A,B&minute=M | 実装済み / parentとchildを共通時刻・レイヤー・ズームで比較 |
| 詳細（記録・モデル・係数・出典・作用・LLMの利用） | GET R、GET R/evidence、GET B/llm-status、GET P/contracts、GET R/usage | 「詳細」の画面。シナリオの記録（条件の版とhash、seed、モデルの版の署名）、分岐の系譜、evidenceのモデルと係数（根拠の等級・値・単位・範囲・出典）、出典と仮定、介入で使える作用の型、LLMの利用を出す。AGENTS.mdの「provenanceを対応する参照画面に残す」に当たる。ブラウザー版は`contracts`・`usage`に答えないので、作用の型と呼び出しの数の代わりにその旨を出す（#376） |
| シナリオの削除、同じ条件で新しいシナリオ | POST R/delete（`confirmation`はRunのID）、POST P/runs（同じScenarioとseed） | シナリオ設定の「このシナリオを削除」（名前を示して確認）と「同じ条件で新しいシナリオを作る」。自分のProjectのシナリオだけ。計算済みの例は消せない（#376） |
| 過去の質問と回答 | GET B/conversations | レポートを開くと、その分岐でこれまでにした質問と回答（最後の100件）を、質問の欄の上に出す（#376） |
| 2〜8分岐の同時比較 | GET R/compare?branch_ids=A,B,…&minute=M | 分岐比較の画面の「ほかの分岐とも並べる」。選んだ分岐（元のシナリオを左端）を、共通の計算済みの時刻（表示中の時刻が入ればその時刻）の指標の表で並べ、左端との差を出す（#376） |
| 両分岐を先へ計算 | POST R/resumeを順番に実行 | 実装済み / 共通の目標まで計算後compareを取得 |
| KPI差分 | compare.differences | 実装済み / ストレス、累積最大待機時間、稼働比率、未解決支援要請の符号付き差分 |
| 折れ線グラフ | GET B/history?kind=kpis | 実装済み / tick終了時記録を標本化・ページ取得。マップのtick直前境界と区別 |
| Branch report・出典・モデル | GET B/report、GET R/evidence、GET B/llm-status | 実装済み / 途中・未生成・fallback・利用不可を区別 |
| 結果Q&A | POST B/questions | 実装済み / factsとexplanations・出典・制限を区別。offlineは回答生成不可を表示 |
| レポート保存 | 上記データからJSON書出し | ブラウザー実装 / 記録とモデル根拠を保存 |
| 感染症リスク、情報到達率 | 共通の正式KPIなし | 未提供 / 数値や「高・中・低」を捏造しない |
| PDF出力・チーム共有 | 専用APIなし | 本UIでは提供しない。操作不能な飾りボタンを配置しない |

## 利用の流れ

0. ホームで、計算済みの例を開くか、「新しいシナリオを作る」を押します。ブラウザー版では「このブラウザーで計算する」で、例と同じシナリオを0分から計算できます。
1. 「例文を使う」か、基本設定の自治体・災害・人数を入力し、期間などを任意指定します。項目指定を優先します。未入力の基本条件は既存parserで文章から抽出します。基本項目が一部だけの場合は抽出プレビューと統合後の最終プレビューを順番に取得します。
2. 「次へ」で入力値・補完値・出典・警告を確認します。公的基準への適合は主張しません。承認ブロッカーがある場合は条件を修正し、再プレビューします。
3. 確認チェックを選択して承認するとRunを作成します。計算目標を経過分で指定して計算します。短い動作確認には100人・0.025日（36分）を利用できます。長期・大人数は計算と保存量が増えます。
4. 再生・シークで計算済み時刻を確認し、「この時刻から分岐を作る」で分岐比較の画面へ移ります。分岐比較の画面では、比べている2つの分岐のAの、表示中の時刻からも分岐を作れます（#374）。介入文を入力します。追加先が曖昧な場合は、APIが返した候補から明示的に選択します。選択結果を `zone_id: ...` として指示へ追記し、作用を再プレビューします。作用を確認・承認すると独立分岐が生成されます。
5. 分岐比較の「両分岐を指定時刻まで計算」で両方を共通目標へ進めます。比較画面の計算目標（経過分）を指定します。シミュレーション画面の目標欄とも同期します。未指定時は両者の先へ60分（期間上限まで）進めます。
6. レポート画面で記録の時系列、人物状態、イベント、出典を確認します。最後まで計算するとBranch reportが生成されます。結果Q&Aと自然言語機能はClaude APIを呼び出します。API接続、利用枠、応答検証で失敗した場合はfallbackせず、エラー理由を表示します。

## 設計・表示上の境界

- 主ブランドは「避難所IF！」。青 `#0878ff`、濃紺 `#102766`、背景 `#edf7fd`、緑 `#00a995`、注意色 `#ff365e`。日本語システムフォントと数値用Avenir/Arialを利用し、CDNは不要です。
- 添付参照のヘッダー・左ナビ・カード密度・青い操作・左右分岐比較を踏襲します。標準施設は明示した模式図で自然な部屋配置へ整理し、実寸や移動の確認には実座標へ切り替えます。床の木目と人物トークンは表示上の演出であり、建物や人物行動をモデルへ追加しません。
- 写実的な建物写真、未記録の人物の声、根拠のない改善予測・感染リスク、実行時間・費用の推定値は生成しません。
- 実座標表示では、スタッフ位置がなければ推測配置しません。模式表示では所属に対応する記号として表示し、物理座標の記録とは区別します。
- 履歴は保存範囲の標本です。グラフの線は記録点同士を結ぶ表示であり、未計算時刻を生成しません。
- マップはドラッグ、矢印キーで移動し、Ctrl/⌘＋ホイールで拡大縮小できます。Homeは表示リセット、Escapeは人物選択解除です。比較の左右は同じパン・ズームを共有します。
- 避難者数の時系列は現APIのKPI履歴に含まれないため、グラフ選択肢に欠測理由を示します。通常表示はストレスとスタッフ負荷の実履歴です。
- 介入文の補助表示は当該ブラウザーのsessionStorageを利用します。復元できない場合は欠測を明示し、APIのCheckpoint分岐根拠を表示します。
- UIのサマリーは記録値と一般的な検討案です。AIによる因果説明や人物の実発言として表示しません。
- 新しい永続スキーマ・係数・モデルは導入しません。承認・分岐・再現性の検証は既存APIへ委ね、UI側でも条件変更時の承認無効化を行います。

## 検証

通常のpytestはofflineで実行します。実APIの有効性確認には、ローカルに設定したAPI keyと、既存の上限付きlive integration testを使用します。デモ起動とは別工程です。

```sh
SHELTERSIM_LLM_MODE=offline PYTHONPATH=src python -m pytest \
  tests/integration/test_web_routes.py tests/unit/web/test_server.py \
  tests/integration/test_demo_v2_routes.py tests/unit/storage/test_projects.py \
  tests/unit/web/test_results_chat.py tests/integration/test_results_chat.py
```

ブラウザーE2Eは `scripts/test_demo_v2.cjs` を参照してください。Playwrightを含むNode環境とChromeを使用し、上記offlineサーバーを起動してから実行します。実API・一時Projectでシナリオ→承認→計算→シーク→介入→分岐→比較→レポート→質問を確認し、1440×900 / 1920×1080の各画面スクリーンショットを出力します。生成データ・画像はGitに登録しません。

明示Zone IDは自然言語の一般名詞との一致より優先します。`zone_id: toilet`、`zone_id: staff` など既存のIDを誤って不明扱いしないように対象解決だけを修正しました。ID未指定の曖昧さと、未知IDの拒否は維持します。

## このブランチで実施した検証

- Python: UI配信、parser、既存Web、永続化、Q&A、Checkpoint、EffectProgram、厳密timelineのofflineテスト95件が通過。
- Chrome headless: 実HTTP APIによるE2E、承認無効化、巻戻し、自然言語の曖昧性解消、元Branchのhash不変、両Branchの計算・同期再生、report取得、質問UI、JSON保存、復元が通過（offline modeによる既存検証）。
- 表示: 1440×900 / 1920×1080の4画面を撮影し目視確認。800px / 390pxでも横方向にはみ出さないことを確認。
- JS構文チェックとgit diff --checkが通過。新規Pythonテストはruffを通過。既存のserver.py（13件）とnatural_language.py（14件）のlint指摘は起点mainと同じで、新規指摘はありません。
- 上記pytestはoffline専用です。live APIの確認は利用者が明示した予算上限と`SHELTERSIM_ALLOW_LIVE_API_TESTS=1`を使う別コマンドで実施します。長期7日・大人数での性能は未検証です。

以下は回帰テスト専用の隔離サーバーの起動例です。製品デモは冒頭のAPI接続設定を使用してください。

```sh
cd /Users/mizuki/.codex/worktrees/ui-backup-20261025/tkb_2615
PYTHONPATH=src SHELTERSIM_LLM_MODE=offline \
  /Users/mizuki/Dev/tkb_2615/.venv/bin/python -m sheltersim web \
  --runs-dir /private/tmp/shelter-ui-backup-test-data --host 127.0.0.1 --port 8877
```

作業ブランチ: `feature/ui-backup-20261025`。起点: `origin/main` の `0d3a641d6a8470085b49e09cfda4a8c2cbe467a5`。既存ディレクトリの未コミット変更は引き継がず、独立worktreeで作業しました。
