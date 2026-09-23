# フレームワークがフェーズを管理する実装への移行

作業ブランチ: `codex/render-interface`。main・他の既存ブランチ・GitHub は変更していない。
この報告は `native-render-interface-report.md` のウィジェット連携部分を更新する。

## 実装した責任分担

ウィジェットの `RenderItem` は `Fn(&RenderCtx, &mut Draw)` を保持する。
描画時に最終フレームへ Object と mask を直接書き込み、ローカル Scene やフェーズ番号を返さない。
旧 `RenderItem::dynamic`、Scene キャッシュ、`append_scene` / `append_local` は撤去した。

```rust,ignore
// texture は内容が変わらない間、同じ定義と ID を保持する。
RenderItem::new(move |ctx, draw| {
    draw.quad(&texture, ctx.size, Matrix4::identity(), None);
})
```

| 所有者 | 所有・処理するもの |
|---|---|
| ウィジェット/provider | 軽量な描画 writer、文字レイアウト・画像・形状などの再利用資源 |
| フレームワークの `Frame` | 最終 Scene、再利用する Vec/ResourcePool、フェーズ境界、座標・clip・opacity の解決 |
| SceneRenderer | 内容 ID による GPU キャッシュ、テクスチャページ・メッシュ配置、最終合成 |
| アプリケーション側 | wgpu のエラー通知・処理方針。同期 Result に非同期エラーの解決を要求しない |

Object ごとの Arc・借用ラッパー・保持キャッシュは追加していない。値を最終配列へ直接書く。
抽出済みフレームが builder を保持するための **ウィジェットごとの一つの Arc** は残す。
invalidate は Scene 用 Arc を再確保せず、値の revision を更新する。変更検知テストも revision に移行した。
資源定義は既存の Arc<Prepare> で provider キャッシュと最終 ResourcePool の間で共有する。
これは Object/Scene の保持とは別であり、所有方式の性能比較が完了したという意味ではない。

## フェーズを番号ではなく描画の意味から作る

`Draw::object` は通常描画、`Draw::backdrop` は **その描画位置より前の背景を読む描画**。
フレームワークが backdrop の直前に必要なフェーズ境界を置く。呼び出し側に境界操作や番号を公開しない。

例えば A の通常描画・背景効果、B の通常描画・背景効果を順に提出すると、

```text
Phase 0: A の通常描画
Phase 1: A の背景効果 → B の通常描画
Phase 2: B の背景効果
```

B の背景効果は A の背景効果を含む画像を読む。旧方式の「全ウィジェットの同番号フェーズを結合」では
この順序を表せなかった。通常描画も含め、UI の paint order を勝手に入れ替えない。

これはフレームワークの一つの明確な背景参照規則であり、任意の効果の意図を自動推測する機構ではない。
複数効果が共通の凍結背景を読むグループや、任意の資源依存 DAG は今回導入していない。
生成器内で私的な Render/Compute 複数パスを記録する機能は引き続き使える。

snapshot 依存フラグは保留のままなので、背景が変わる出力には新しい ID が必要。
`Draw::backdrop` は内容 ID の意味を変更したり、同じ ID の中身を黙って更新したりしない。

## 座標・マスクとキャッシュ

`Draw::translated` / `Draw::masked` はその場で描画を実行するスコープ。別の描画木や Scene を保存しない。
mask は任意メッシュを使え、親から継承するのは被覆のみ。兄弟へ出るとスコープが復元される。
`Draw::transform()` は入れ子の配置を含む解決済み行列を返し、背景生成器が読む座標に利用できる。
opacity は引き続き Object ごとの乗算であり、隔離したグループ透明度ではない。

ResourcePool は未変更の定義を保持し、各提出で参照・登録されたものを残す。未使用登録は保持ヒント。
構築失敗でも、その提出に含まれなかった古い CPU 定義を掃除する。GPU キャッシュの予算方針は変更しない。

標準ウィジェットをすべて Draw に移行した。Text/RichText は writer 内で wrap width ごとの現在の
文字レイアウトを保持し、Button は最初の描画時にラベルを整形して保持する。TextBox は editor の
レイアウトを使う。画像 decode/resize と GPU shape の生成定義も引き続き再利用する。
RichText の色定義は writer と同じ寿命にして、フォントコンテキストへ色を蓄積させない。

## 実装時に見つけたこと

単に毎フレーム builder を実行するだけでは、描画レコード以外の準備も再実行される。
最初の計測では静止 showcase の組み立てが約 2 ms、CPU 確保 19 回＋再確保 1 回だった。
swash の scaler 構築が glyph キャッシュヒット時にも行われていたため、ミス時だけに移した。
角丸 border の一時 Vec と、quad の生成器を描画ごとに clone する処理も不要になった。

計測は examples 専用の allocator で **組み立て中の実行スレッドだけ**を数える。
GPU 記録・ドライバ・他スレッドの確保や、live memory / VRAM の計測ではない。
本番ライブラリに allocator は追加していない。

## 再現

全 workspace テストは **460 成功 / 0 失敗 / 既存 ignored 8**。全 examples のビルドも成功。
Vulkan / DX12 の双方で次を確認した。

- 重なる別ウィジェットの背景効果が、前の効果を含む画像を読む。
- 効果の間・後にある通常描画が paint order を守る。
- 移動後の背景座標、キャッシュ消去後の再生成、GPU 配置変更後の画素が正しい。
- GPU 形状 5 種と CPU オラクルの最大差が 0。
- 20 回の不正な構築が CPU 定義を蓄積せず、修復後に元の画素へ戻る。

showcase は旧実装結果と **900,000 / 900,000 画素一致**。CPU 組み立ての確保回数と
release 計測、両バックエンドの出力記録は [validation.txt](framework-draw/validation.txt) に保存する。
比較画像は [従来と一致した showcase](native-render-interface/showcase.png)、背景の順序は
[通常配置](framework-draw/paint-order.png) / [移動後](framework-draw/paint-order-moved.png) を参照。

最終 release 計測（AMD Radeon RX 5700 XT / Vulkan、142 UI items / 1,038 Object draws）:

| 項目 | 結果 |
|---|---|
| 静止フレームの CPU 組み立て | 0.499 / 0.455 / 0.451 ms |
| 同区間のヒープ確保・再確保 | 3 フレームとも 0 / 0 |
| 静止フレームの GPU 資源生成 | 0 |
| 静止フレーム全体（GPU 完了待ち込み） | 4.61 / 3.40 / 3.44 ms |
| 最初のフレーム | 約 246 ms（文字・画像等の準備を含む） |

数字はローカルの小標本であり、FPS 保証ではない。CPU 確保ゼロもこの静止 showcase の計測範囲についての結果。

保持済み Scene を再利用する旧方式の過去の組み立て記録は約 0.13 ms。今回の軽量 writer 方式は
描画レコードの保持を減らす一方、再構築の CPU 処理は増える。異なる所有方式を同時条件で網羅的に
比較したベンチマークではなく、全ウィジェット・全更新パターンでの速度優位は主張しない。

```powershell
cargo build --workspace --examples --offline -j 2
cargo test --workspace --exclude shared-buffer --offline -j 2
$env:MATCHA_TEST_BACKEND='vulkan'
cargo run -p matcha-ecs --example interface_stress --offline -j 2 -- target/framework-vulkan
$env:MATCHA_TEST_BACKEND='dx12'
cargo run -p matcha-ecs --example interface_stress --offline -j 2 -- target/framework-dx12
Remove-Item Env:MATCHA_TEST_BACKEND
cargo run -p matcha-ecs --example showcase --release --offline -j 2 -- --offscreen target/framework-showcase.png
cargo run -p renderer --example image_diff --offline -j 2 -- docs/native-render-interface/showcase.png target/framework-showcase.png
```

sampler-policy、snapshot フラグ、ページ容量の最適化、既存上流キャッシュの寿命方針は今回変更していない。
wgpu の同期・非同期エラーを統合する新 API も追加していない。従来の通知経路を維持する。
ブラウザ、実 IME、手操作の連続リサイズは今回の検証に含めない。
