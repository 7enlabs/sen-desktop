# Build notes - scaffold B-2, bundle local chua day

Ngay build: 03/11/2026, trong ca lam viec. Trang thai: KHONG PHAI DONE. Chua co kiem chung doc lap.

## 1. Can cu va head nhanh

- Cong dung: tin Telegram 4064 ngay 03/11/2026, SD Leader mo cong dung B-2 bang van ban, tag @sddeveloper_bot. Huong: build theo file prep cong toan van da xac nhan, qua nhanh cong PR, qua review ZC Leader, khong tu merge.
- Head nhanh dev/scaffold: 4564a9c la theo kiem chung cua Leader o tin 4055 va nhac lai o tin 4064.
- CHUA KIEM CHUNG LAI bang nguon doc lap: trong workspace nay khong co clone repo, luot nay khong tao ket noi, khong chay lenh kiem tra head tuoi. So 4564a9c o day chi la dan lai tu tin Leader, khong phai ket qua kiem tra tuoi cua luot nay.
- Khong day, khong commit len remote, khong mo PR, khong merge, khong gui gi len Telegram trong luot nay.

## 2. Da tao gi

Khung repo pnpm, ESM-only, theo prep muc 1:

- package.json goc, pnpm-workspace.yaml, .gitattributes (LF), .editorconfig, README.md.
- apps/desktop: package.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json, src-tauri/src/main.rs, src-tauri/src/commands/mod.rs, session.rs, tools.rs, events.rs, src/ipc/types.ts, src/ipc/bridge.ts, src/app/main.ts, src/app/index.html.
- packages/core: package.json ghim Pi SDK 1.3.0, src/index.ts, session.ts, raw-log.ts, execution-gate.ts, token-economizer.ts, paths.ts.
- packages/providers, tools, prompts, ui: moi package co package.json va src/index.ts.
- evals/README.md, docs/b2-checklist.md, build-notes.md (file nay).

## 3. File nao co noi dung that, file nao la khung hoac stub

Co noi dung that ve cau truc va logic thuan tuy, nhung CHUA CHAY, CHUA BUILD, CHUA KIEM TRA TU DONG:

- apps/desktop/src/ipc/types.ts: kieu hop dong hanh dong diem 888 that ve truong du lieu.
- apps/desktop/src/ipc/bridge.ts: ham kiem tra hop dong va chan thieu log tho (A1) that ve logic, phan goi Tauri invoke that thi STUB.
- packages/core/src/session.ts: statechart diem 854 that ve bang chuyen trang thai, phan goi createAgentSession that thi STUB.
- packages/core/src/raw-log.ts: lop log tho that ve cau truc, chua co du lieu that, chua co dong kin AfterDeleteBench.
- packages/core/src/execution-gate.ts: thu tu 6 chang that, cac chang kiem tra that thi STUB.
- packages/core/src/token-economizer.ts: tieu chi dung turn diem 856 that ve hang so va ham xet, phan phat hien that thi STUB.
- packages/core/src/paths.ts: mot bien duong dan that ve co che, chua co root that.
- packages/core/package.json: ghim 1.3.0 that ve van ban khai bao, chua cai, chua doi chieu lai registry.

Khung hoac stub, chua co code that chay duoc:

- Toan bo src-tauri: main.rs, commands/session.rs, tools.rs, events.rs, tauri.conf.json, Cargo.toml.
- apps/desktop/src/app/main.ts va index.html: vo giao dien chua lap day.
- packages/providers/src/index.ts: ModelRuntime chua lap day, model mac dinh KHONG CO theo tin 4070.
- packages/tools/src/index.ts: khung hook tool_call, logic xet quyen that chua lap day.
- packages/prompts/src/index.ts va packages/ui/src/index.ts: ResourceLoader va renderer chua lap day.
- evals: chua co ca eval that nao.

## 4. Rang buoc da giu

- Pi SDK ghim 1.3.0, khong nang len 1.4.0. Khong co cua-driver, khong co muc iOS, khong co F8 MCP, khong bundle font, khong co TrustBoost, NVISO, agent-memory-guard Python, Decepticon.
- Khong co lenh shell phu thuoc he dieu hanh o tang khung, khong ghep duong dan bang chuoi, khong gia dinh thu muc nha.
- Khong dung credential, key, token. Khong tham do ban tra phi. Khong cai package trong luot nay.

## 5. Con mo va dieu kien chua du

- Head tuoi nhanh dev/scaffold CHUA KIEM CHUNG LAI bang nguon doc lap (xem muc 1).
- Toan van khung B-S2 CHUA CO. Toan van dai quyet B-S1 den B-S8 CHUA CO. Neu xuat hien sau thi doi chieu ngay tung muc prep, lech thi sua truoc khi merge.
- Toan van phieu ra tin 1059, checklist tin 965, ap dung som tin 972 CHUA CO, chi thay caption.
- Version Tauri, identifier, quyen toi thieu, vo desktop, model mac dinh: xem muc 6, da chot theo tin 4070; patch Tauri exact + lockfile cho luot lap sau review.
- Pi SDK CHUA CAI, CHUA DOI CHIEU lai license MIT tren registry trong luot nay, goi createAgentSession that CHUA XAC MINH thuoc package nao trong hai package.
- Dieu kien mo A1-A4 CHUA XAC MINH: Pi SDK 1.3.0 co loi goi phu dang do hay khong.
- F8 MCP cho doi chieu toan van B-1 va STU-2. F5 cua-driver hoan toi sau cong Z Red 10/11.
- Bundle nay CHUA BUILD, CHUA CHAY, CHUA CO BANG CHUNG CHAY DUOC. Moi tuyen bo ve chay duoc deu la sai neu viet them.

## 6. Cap nhat theo chot tham so tin 4070 (luot 18:40 ngay 03/11/2026)

- ZC Leader tin 4070 chot: Tauri version 2; identifier dev.7enlab.sen-desktop (tam); quyen toi thieu chi core Tauri, chua them plugin; cua so chinh rong 1200x800, tieu de tam Pi Desktop toi 10/11 xac minh ten Sen; model mac dinh KHONG CO, providers chi la vo khai bao kenh.
- Head tuoi: ZC Leader kiem chung truc tiep luc 18:35 tin 4070, nhanh dev/scaffold van 4564a9c, khop prep. Ghe van CHUA KIEM CHUNG LAI bang nguon doc lap; so head nay la dan lai tu tin Leader.
- Da sua trong bundle, van giu dung 35 file, khong them file: Cargo.toml ghi Tauri 2 (patch exact + lockfile cho luot lap sau review); tauri.conf.json ghi identifier, cua so 1200x800, tieu de Pi Desktop, quyen chi core; index.html doi tieu de sang Pi Desktop; providers/index.ts bo truong model mac dinh, ghi ro KHONG CO model.
- Van CHUA BUILD, CHUA CAI package, CHUA LAP SDK that. Trinh tu tin 4070 giu nguyen: Leader review cau truc truoc, roi ghe lap SDK that + build kiem tra, du moi quyet merge.
- Diem mo cu o muc 5 da dong mot phan: version Tauri (major), identifier, quyen, vo desktop, model mac dinh da chot theo tin 4070. Toan van B-S2, dai B-S1..B-S8, phieu ra 1059, checklist 965, ap dung som 972 van CHUA CO; dieu khoan doi chieu khi ban goc xuat hien van giu.

## 7. Buoc tiep theo can Leader review

1. Leader day bundle nay len nhanh moi + mo PR de review cau truc (khong day thang dev/scaffold, khong tu merge). Head goc dev/scaffold theo kiem chung Leader tin 4070 la 4564a9c.
2. Tham so cau truc da chot tin 4070 (xem muc 6); ban goc xuat hien sau thi doi chieu ngay, lech sua truoc khi merge.
3. Leader cap toan van B-S2, dai B-S1 den B-S8, phieu ra tin 1059, checklist tin 965, ap dung som tin 972 de doi chieu truoc khi chot build.
4. Sau khi co cac muc tren: cai package Pi SDK 1.3.0, doi chieu license MIT, lap day import SDK that, lap day cac chang cong thuc thi, roi moi build va chay kiem tra.
5. Nop qua nhanh cong PR de ZC Leader review, khong tu merge. Kiem chung phai do nguoi khac lam, khong phai nguoi san xuat bundle nay.
