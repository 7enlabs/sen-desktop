# Checklist wiring Pi SDK 1.3.0 - trang thai scaffold B-2

Nguon: prep B-2 muc 3. Trang thai duoi day la trang thai that cua bundle local nay, khong phai trang thai mong muon.

- [ ] 1. Ghim dung 1.3.0 cho ca hai package: DA GHI trong packages/core/package.json, CHUA CAI, CHUA DOI CHIEU lai npm registry trong luot build nay.
- [ ] 2. License MIT cho ca hai package: DA GHI trong package.json, CHUA DOI CHIEU lai npm registry trong luot build nay. Scaffold mang thong bao MIT o cac package.json.
- [ ] 3. apps/desktop ESM-only: DA GHI type module, CHUA KIEM TRA bang build vi chua cai package.
- [ ] 4. packages/core boc createAgentSession: CHUA CO import SDK that. Chi co khung SessionManager va statechart diem 854.
- [ ] 5. packages/providers khoa version Pi: DA GHI hang so 1.3.0, ModelRuntime CHUA LAP DAY, model mac dinh KHONG CO theo chot tin 4070, providers chi la vo khai bao kenh.
- [ ] 6. packages/tools lop permission: DA CO khung hook tool_call, logic xet quyen that CHUA LAP DAY.
- [ ] 7. Tauri invoke + IPC bridge: DA CO kieu hop dong va ham kiem tra hop dong, invoke that CHUA CO; Tauri version 2 chot major theo tin 4070, patch exact + lockfile dien o luot lap sau review cau truc.
- [ ] 8. Log su kien tho: DA CO lop RawLog that ve cau truc, CHUA CO du lieu that, CHUA CO dong kin AfterDeleteBench.
- [ ] 9. Cong thuc thi duy nhat 6 chang: DA CO thu tu va ham kiem tra thu tu, cac chang that CHUA LAP DAY.
- [ ] 10. Khong ap cac che do bi cam: DA GHI cam trong code tools, CHUA KIEM TRA bang grep tu dong trong luot nay.

Dieu kien dung, theo prep muc 3: dung ngay neu version SDK khac 1.3.0, license khac MIT, duong goi tat ngoai cong, thieu log tho, yeu cau cua-driver truoc cong Z Red 10/11, yeu cau iOS truoc quyet moi. Sau hai lan that bai o cung mot buoc thi dung va lap lai ke hoach.

Diem A1-A4: A1 thieu log thi FAIL, da ghi trong bridge. A2 loi goi model chi phuc vu hien thi mac dinh TAT, CHUA CO co che tat that. A3 cam cong va runtime ben thu ba, CHUA KIEM TRA. A4 bao cao ngan sach token theo ca tach rieng cot loi goi phu, CHUA CO. Dieu kien mo A1-A4: Pi SDK 1.3.0 co loi goi phu dang nay hay khong CHUA XAC MINH.
