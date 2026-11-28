# デプロイナウ版のビルドと公開

同じソースから、Cloudflare Workers 版とロリポップ！デプロイナウ版を作ります。デプロイナウ版は、静的サイトの配信で画面・Web Worker・Rust から生成した WASM を読み込み、計算と OPFS への保存をブラウザー内で行います。新規シナリオの地域調査では、同梱した全国の指定避難所の索引・自治体別データと、国土地理院の住所検索を使います。

| 項目 | デプロイナウ版 | Cloudflare Workers 版 |
| --- | --- | --- |
| 組み立て | `--profile deploy-now` | 既定の `--profile cloudflare` |
| 計算済みの例 | 江戸川区の水害（100 人、5 日間、通常運営と 12 時間の断水） | 港区・墨田区・江戸川区の 3 件 |
| LLM | 無効。合言葉、API 公開モード、文章からの条件設定・介入、結果への質問、Opus の考察を省く | 合言葉・API 公開モード・中継用管理画面を使用 |
| 共通機能 | 手動の条件設定・地域調査、地図と時系列再生、計算の中断・再開、手動介入、分岐比較、記録に基づくレポートと PDF 保存 | 同左 |

江戸川区の例は、100 人の避難生活で給水条件が変わったときの行動や指標を、同じ時刻の A/B 比較ですぐに見られるように選びました。例を選んだときだけ分割ファイルを取得し、チェックサムを照合してから OPFS に保存します。

OPFS は公開先のオリジンごとに分かれます。Cloudflare 版の保存データはデプロイナウ版には自動で引き継がれません。同じ公開 URL にデプロイナウ版を再配信すると、そのオリジンの保存領域を引き続き使用します。

## GitHub Actions で作る

1. リポジトリーの **Actions → デプロイナウ版のビルド → Run workflow** を開き、公開する版のブランチまたはタグを選びます。ワークフローを追加・変更するプルリクエストでもビルドします。初回の手動実行は、GitHub の既定ブランチにワークフローが登録されてから可能になります。
2. 実行結果の **Artifacts → deploy-now-site** をダウンロードして展開します。展開先の直下に `index.html`、`browser/`、`sample/` が並ぶフォルダーが公開物です。
3. **deploy-now-build-info** の `site-build.json` で、合計バイト数、ファイル数、最大ファイル、元コミット、避難所索引とサンプルの情報を確認します。ビルドは 100,000,000 バイト未満を要求します。

このワークフローは、GitHub Actions 上でネイティブカーネル、WASM、江戸川区の例、全国データを生成します。保存した成果物を、Chromium と Firefox で確認します。公開は次の手順で行います。GitHub Actions にデプロイナウの認証情報を登録する必要はありません。

## 手元で作る

Python 3.12 以上、Rust stable、`wasm32-unknown-unknown` ターゲット、WASM クレートと同じ版の `wasm-bindgen` CLI が必要です。CI が検証する CLI の版と SHA-256 はワークフローを参照してください。

リポジトリー直下の Bash で実行します。

```bash
python3.12 -m venv .venv
source .venv/bin/activate
python -m pip install -e . 'maturin>=1.7,<2'
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
work="$(mktemp -d)"

maturin build --locked --profile ci -m native/Cargo.toml --out "$work/wheels"
python -m pip install --no-index --find-links "$work/wheels" sheltersim-native
cargo build --locked --manifest-path native/Cargo.toml -p sheltersim_wasm \
  --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir "$work/wasm-web" \
  native/target/wasm32-unknown-unknown/release/sheltersim_wasm.wasm
SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py "$work/projects" \
  --example sample-3 --bundles "$work/samples"
python scripts/build_static_site.py "$work/site" --profile deploy-now --pkg "$work/wasm-web" \
  --bundle "$work/samples/sample-3.shelterproj" --fetch-shelters > "$work/site-build.json"
cat "$work/site-build.json"
python scripts/serve_static_site.py "$work/site" --port 3000
```

`http://127.0.0.1:3000/` で確認できます。すでに取得済みの全国版 `mergeFromCity_1.csv` を使う場合は、`--fetch-shelters` を `--shelter-csv /path/to/mergeFromCity_1.csv` に置き換えます。既知の取得日時は `--shelter-retrieved-at` に指定できます。公開ビルドには合成のテスト用 CSV の代わりに、全国版を使用してください。[国土地理院の配布ページ](https://hinanmap.gsi.go.jp/index.html)

## Free プランで公開する

CLI はフォルダーの構成を維持し、同じプロジェクトへの更新にも使えます。管理画面からフォルダーをアップロードする方法も選べます。Actions の ZIP はダウンロード用なので展開して、`index.html` が直下にあるフォルダーを選びます。GitHub 連携はブランチの push からビルド・配信する方式です。この構成では Actions が出力を作り、CLI または管理画面で直接アップロードするため、生成物用ブランチや大容量ファイルのコミットを必要としません。[静的サイトの公式ガイド](https://deploy.lolipop.jp/docs/frameworks/static) · [GitHub 連携](https://deploy.lolipop.jp/docs/integrations/github) · [ブラウザーのアップロード手順](https://deploy.lolipop.jp/docs/quickstart/browser)

公式 CLI には Node.js 22.12.0 以上が必要です。Free プランのアカウントでログインし、未使用の名前・サブドメインを選びます。

```bash
npm install -g lolipop
lolipop login
lolipop deploy --name hinanjo-if --framework static --dir /path/to/deploy-now-site
```

管理画面を使う場合は、**新規作成**で展開済みフォルダーを指定し、フレームワークを**静的サイト**に設定します。プロジェクト名と公開 URL を入力して**デプロイ**を押します。公開ルートはフォルダー直下の `index.html` です。インストール・ビルドコマンド、環境変数、GitHub 連携の入力は不要です。HTTPS はデプロイナウが提供します。公開後に管理画面でプロジェクト ID と HTTPS URL を控えます。[CLI リファレンス](https://deploy.lolipop.jp/docs/cli) · [管理画面の手順](https://deploy.lolipop.jp/docs/quickstart/browser)

## 公開後の確認

1. HTTPS のルートと `/version.json` を開き、作成したコミットを確認します。次のパスを `curl -I` かブラウザーのネットワーク欄で確認します。

   | パス | 期待する応答 |
   | --- | --- |
   | `/` | 200、`text/html` |
   | `/demo-v2.js`、`/browser/api_worker.mjs?llm=disabled` | 200、JavaScript の MIME タイプ |
   | `/browser/pkg/sheltersim_wasm_bg.wasm` | 200、`application/wasm` |
   | `/sample/sample-3.json`、`/browser/gsi-shelter-index.json` | 200、JSON |
   | `/sample/sample-3.shelterproj.000` | 200、分割ファイルのバイト列 |
   | `/admin`、`/admin.html`、`/missing-file.mjs` | 404 |

2. Chromium と Firefox の通常のウィンドウで「計算済みの例を取得する」を選びます。読み込み後に再生・巻き戻し・分岐比較を試し、`/?project=sample-3&run=run-edogawa-water-outage&branch=main#simulation` を再読み込みして保存済みの例が開くことを確認します。
3. 新しいシナリオを作り、自治体の条件と公開データの出典、計算、中断・再開、手動介入、分岐比較、レポートを確認します。レポート保存から印刷画面を開き、「PDF に保存」を選びます。
4. ネットワーク欄で `/llm/*` が 0 件か、地域調査時の国土地理院への接続や Worker の通信が CSP・CORS で拒否されていないか、初回アクセス時に 429 やアセット取得失敗がないかを確認します。静的配信の MIME、404、CSP と制限の適用は公開先の応答で確認します。

Playwright がある環境では、画面の確認を次のように実行できます。

```bash
python -m pip install playwright
python -m playwright install chromium firefox
SHELTERSIM_LLM_MODE=offline python benchmarks/static_site_e2e.py \
  --url https://YOUR-SUBDOMAIN.lolipop-now.app --deploy-now --require-all
```

ローカルで Python 版との同値性も確認する場合は、同じ CSV で組み立てたサイトについて `--site "$work/site" --work "$work/e2e" --shelter-csv /path/to/mergeFromCity_1.csv` を `--url` の代わりに指定します。このモードは国土地理院の住所検索の記録済み応答を使います。

## 更新とロールバック

`main` を更新したら Actions で新しいコミットをビルドし、同じプロジェクトへ展開済みの成果物を送ります。

```bash
lolipop deploy --project PROJECT_ID --dir /path/to/deploy-now-site
```

管理画面ならプロジェクトの **デプロイ → 新しいバージョンをデプロイ** で同じ操作ができます。問題があれば、控えておいた前回正常版の成果物を同じプロジェクトに再アップロードし、`/version.json` と画面を確認します。Actions の成果物には保存期限があるので、公開した版の ZIP と `site-build.json` を手元に保管しておきます。[管理画面の更新手順](https://deploy.lolipop.jp/docs/quickstart/browser)

## 無料枠

2026 年 11 月 28 日時点の公式プラン表は、CLI・ファイルアップロードが 100 MB、展開後は 10 GiB、CDN 帯域は月 100 GB、月間リクエスト数は 100 万、ピークは秒間 10 リクエストとしています。サンプルの分割ファイルは、利用者が例を選んだ時点で取得します。端末の OPFS が例を保持するため、再生のたびにサンプル全体を再取得しません。ピーク制限の CDN キャッシュへの適用はプラン表に記載がないため、実公開後に初回ロードの応答と管理画面の使用量を確認します。[公式プラン表](https://deploy.lolipop.jp/docs/plans)
