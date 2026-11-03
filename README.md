# Sen Desktop - scaffold B-2

Trang thai: KHUNG CHUA BUILD. Day la bundle local, chua day len remote, chua mo PR, chua merge.

Can cu: file prep da duyet `b2-scaffold-kickoff-prep-2026-11-03.md`, cong dung SD Leader tin Telegram 4064 ngay 03/11/2026, xac nhan head cua ZC Leader tin 4055, chot tham so ZC Leader tin 4070 ngay 03/11/2026.

Tham so chot tin 4070, da dien vao bundle o muc cau truc:

- Tauri version 2 (major chot, cung major voi tuyen Desktop); patch exact + Cargo.lock dien o luot lap SDK that sau review cau truc.
- Identifier: dev.7enlab.sen-desktop (tam, doi duoc truoc khi phat hanh).
- Quyen toi thieu: chi core Tauri, chua them plugin nao.
- Vo desktop: cua so chinh rong, 1200x800, tieu de tam Pi Desktop toi 10/11 xac minh ten Sen theo B-1, khong UI tinh nang, khong noi dung gia.
- Model mac dinh: KHONG CO; packages/providers chi la vo khai bao kenh, chua gan model, chua key, chua goi mang.

Khung repo: pnpm workspace, ESM-only. `apps/desktop` khong chua logic loi dung chung.

Cac rang buoc giu nguyen tu prep:

- Pi SDK ghim 1.3.0, khong nang len 1.4.0.
- Khong dua cua-driver vao truoc cong Z Red 10/11. Khong co muc iOS trong scaffold nay.
- F8 MCP cho doi chieu toan van, khong co trong scaffold nay.
- Khong bundle font khi chua mo file license. Khong co thu muc font trong scaffold nay.
- Khong co TrustBoost, NVISO, agent-memory-guard Python, Decepticon lam dependency.
- Khong co `require()` goi SDK. Khong co lenh shell phu thuoc he dieu hanh o tang khung.
- Moi hanh dong thay doi the gioi ben ngoai chi qua cong thuc thi duy nhat 6 chang trong `packages/core/src/execution-gate.ts`.

Xem chi tiet trang thai that cua tung file trong `build-notes.md`. File nao ghi STUB thi chua co code that chay duoc, du van ban trong file la that.
