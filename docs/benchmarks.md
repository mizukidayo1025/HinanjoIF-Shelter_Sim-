# オフライン性能計測

この計測器はProviderやネットワークを使わず、合成Scenarioでシミュレーションと成果物の書き込み時間、プロセスのピークRSS、記録量を測ります。結果は実装・ホスト・合成条件の記述であり、実際の避難所運用やモデルの妥当性を示しません。

## 中核の計算と保存付き計算

`benchmarks/investigate_performance.py` は、100人のScenarioを中核（`PlainRun`）で計算する時間を、1ケース1プロセスで測ります（#430）。

```shell
PYTHONPATH=src SHELTERSIM_LLM_MODE=offline python benchmarks/investigate_performance.py \
  --scenario generated --duration 10080 --route project --output /tmp/sheltersim-perf-project
```

`--route plain` は `PlainRun` を保存なしで最後まで、`--route project` は画面と同じ道（`ProjectAPI` のworkerが形v6の保存したRunへ、360分ごとに保存する点を書きながら計算する道、#427）を測ります。`--scenario-file` で承認済みScenarioのローカルコピーを指定できます。生Scenarioには人口情報が含まれる場合があるため、出力先はGit管理外にします。ネットワーク接続は計測中に拒否します。

### ピークのメモリー

`result.json` の `peak_rss` に、プロセスの最大RSS（MiB）を記録します（#138）。

| 項目 | 意味 |
|---|---|
| `computation_mib` | 計算が終わった時点の、このプロセスの最大RSS。準備と計算を含み、そのあとの読み直しを含みません |
| `with_hashing_mib` | 終わりの時点の、このプロセスの最大RSS。保存したRunの読み直しを含みます |

値は `resource` モジュールで読むので、Windowsではどれも `null` です。

### ハッシュの比較

`result.json` の `core_digests` は、終わりの世界の部分ごとのdigestと、指標の標本のdigestです。`--expect` に前の計測の `result.json` を渡すと、これを比べます。結果は `result.json` の `expected_hashes` に記録し、違えば終了コード1で終わります。同じScenario・期間で、変更の前後の計算結果が変わっていないことを確かめるときに使います。`plain` と `project` の道は、同じdigestになります。

```shell
PYTHONPATH=src SHELTERSIM_LLM_MODE=offline python benchmarks/investigate_performance.py \
  --scenario generated --duration 10080 --route plain --output /tmp/sheltersim-perf-after \
  --expect /tmp/sheltersim-perf-before/result.json
```

## 保持した1日計測

次の結果は、Pythonの`run_scenario()`の経路（`benchmarks/run_benchmarks.py`。どちらも#430でなくしました）で、コードSHA `19e5cde0a4d59de140582e45b6c9dce3e2b04b7b` の時点で取得した、Linux x86_64・CPython 3.12.14・報告CPU数3の単一seed（2026）結果です。1ケースあたり1440分、trace level `off`、offline modeを使用し、有料API呼び出しはありません。元の作業環境に含まれていたScenarioの絶対パスだけを移管時にリポジトリ相対パスへ置き換えています。値と計測対象は変更していません。

| 人数 | 壁時計 (秒) | CPU (秒) | ピークRSS (MiB) | 成果物 (MiB) |
|---:|---:|---:|---:|---:|
| 30 | 6.29 | 6.28 | 49.8 | 4.27 |
| 100 | 18.09 | 18.08 | 76.7 | 12.15 |
| 500 | 123.71 | 123.56 | 202.1 | 48.44 |
| 1000 | 275.31 | 274.81 | 316.0 | 78.06 |

この結果は各人数につきseedが1つだけなので、seed間のばらつきは分かりません。30人・30日間の別試行は10分以内に完了成果物が得られず停止しました。30日間の完了時間や性能目標は確立していません。

詳細なScenario/state hash、イベント数、trace量、実行環境は[JSON記録](../benchmarks/results/scale-one-day-indexed-2026-10-23.json)と[表形式の記録](../benchmarks/results/scale-one-day-indexed-2026-10-23.md)にあります。過去の性能値とはコードSHAや設定が異なるため、単純な改善率として比較しません。

## 解釈上の制約

- 合成人口、設備数、需要、容量、seedは計測用条件であり、現実の施設構成を表しません。
- ピークRSSは個別プロセスの高水位でPython起動時の分も含みます。壁時計・CPU時間には成果物書き込みを含みます。
- 1000人・30日間、実機上のブラウザー応答性、複数利用者の同時運用は確認されていません。
- CIは小さな計測器のschema/smoke確認を行い、大規模matrixは通常実行しません。
