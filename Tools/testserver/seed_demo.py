#!/usr/bin/env python3
"""Seeds a realistic demo mailbox into the test Dovecot for manual UI testing.

    python3 Tools/testserver/seed_demo.py [user]     (default user: demo@localhost)

Log in with that address and password "test" on 127.0.0.1 (IMAP 31143, SMTP 31587, no encryption).
All people, companies and addresses are made up.
"""

import base64
import os
import sys
import time
from email.utils import format_datetime
from datetime import datetime, timedelta, timezone

ROOT = os.environ.get("LITEMAIL_IT_ROOT") or os.path.join(os.path.dirname(__file__), "..", "..", ".testserver")
USER = sys.argv[1] if len(sys.argv) > 1 else "demo@localhost"
ME = USER
counter = 0


def deliver(folder, raw, days_ago, flags=""):
    global counter
    counter += 1
    base = os.path.join(ROOT, "home", USER, "Maildir")
    d = base if folder == "INBOX" else os.path.join(base, "." + folder)
    for sub in ("cur", "new", "tmp"):
        os.makedirs(os.path.join(d, sub), exist_ok=True)
    name = f"{1700000000 + counter}.demo{counter}.localhost:2,{flags}"
    path = os.path.join(d, "cur", name)
    with open(path, "wb") as f:
        f.write(raw.replace("\r\n", "\n").replace("\n", "\r\n").encode("utf-8"))
    when = time.time() - days_ago * 86400
    os.utime(path, (when, when))


def date(days_ago):
    return format_datetime(datetime.now(timezone.utc) - timedelta(days=days_ago))


def text_mail(frm, subject, body, days_ago, to=None, cc=None, msgid=None, reply_to=None):
    h = [f"From: {frm}", f"To: {to or 'Alex <' + ME + '>'}"]
    if cc:
        h.append(f"Cc: {cc}")
    h += [f"Subject: {subject}", f"Date: {date(days_ago)}", f"Message-ID: <{msgid or f'demo{counter + 1}@example.test'}>"]
    if reply_to:
        h += [f"In-Reply-To: <{reply_to}>", f"References: <{reply_to}>"]
    h += ["MIME-Version: 1.0", "Content-Type: text/plain; charset=utf-8", "Content-Transfer-Encoding: 8bit"]
    return "\n".join(h) + "\n\n" + body + "\n"


PDF = base64.b64encode(b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n").decode()


def with_attachment(frm, subject, body, filename, days_ago, mime="application/pdf", data=PDF):
    return f"""From: {frm}
To: Alex <{ME}>
Subject: {subject}
Date: {date(days_ago)}
Message-ID: <att{counter + 1}@example.test>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="b1"

--b1
Content-Type: text/plain; charset=utf-8

{body}
--b1
Content-Type: {mime}; name="{filename}"
Content-Disposition: attachment; filename="{filename}"
Content-Transfer-Encoding: base64

{data}
--b1--
"""


NEWSLETTER = """From: 山野户外 <news@outdoor.example>
To: {me}
Subject: 秋季新品：为长途旅行准备的轻量装备
Date: {date}
Message-ID: <news1@outdoor.example>
List-Unsubscribe: <https://outdoor.example/unsubscribe?u=123>
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="alt"

--alt
Content-Type: text/plain; charset=utf-8

秋季新品已上架：更轻的羽绒夹克、可折叠登山杖，以及重新设计的 28 升背包。
--alt
Content-Type: text/html; charset=utf-8

<html><body style="margin:0;background:#f3efe8">
<table width="100%" cellpadding="0" cellspacing="0" bgcolor="#f3efe8"><tr><td align="center">
<table width="560" cellpadding="0" cellspacing="0" style="background:#ffffff;border-radius:12px;font-family:Helvetica,Arial,sans-serif;color:#2b2b2b">
<tr><td><img src="https://images.outdoor.example/hero-autumn.jpg" width="560" height="240" alt="秋季新品" style="display:block;border-radius:12px 12px 0 0;background:#c8b9a3"></td></tr>
<tr><td style="padding:28px 32px">
<h1 style="margin:0 0 12px;font-size:24px;color:#3b2f22">为长途旅行准备的轻量装备</h1>
<p style="font-size:15px;line-height:1.6;color:#555">更轻的羽绒夹克、可折叠登山杖，以及重新设计的 28 升背包。本周会员下单享受免费配送。</p>
<p><a href="https://outdoor.example/autumn" style="display:inline-block;background:#3b6e4f;color:#fff;padding:12px 22px;border-radius:24px;text-decoration:none">查看新品</a></p>
<img src="https://track.outdoor.example/open.gif?u=123" width="1" height="1" alt="">
</td></tr></table>
</td></tr></table>
</body></html>
--alt--
"""

PROMO = """From: Northwind Cars <hello@mailer.northwind.example>
To: {me}
Subject: Enjoy these perks when you sell
Date: {date}
Message-ID: <promo1@northwind.example>
List-Unsubscribe: <https://northwind.example/unsubscribe?u=9>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<html><body style="margin:0;background-color:#f4f4f4">
<table width="100%" cellpadding="0" cellspacing="0"><tr><td align="center" style="padding:24px 12px">
<table width="560" cellpadding="0" cellspacing="0" style="background:#ffffff;border-radius:16px;font-family:Helvetica,Arial,sans-serif;color:#222">
<tr><td align="center" style="padding:36px 32px 8px"><img src="https://cdn.northwind.example/logo.png" width="160" height="72" alt="Northwind"></td></tr>
<tr><td align="center" style="padding:12px 40px 32px">
<h1 style="margin:0 0 16px;font-size:28px;line-height:1.2">Sell in 24 hours or less, plus enjoy this package of benefits</h1>
<p style="font-size:16px;line-height:1.6;color:#444">We know life gets busy. Selling your car with us is hassle-free and quick: sell in 24 hours or less <b>and</b> have cash in your account the same day.</p>
<p><a href="https://northwind.example/offer" style="display:inline-block;background:#ef3b3b;color:#fff;padding:14px 40px;border-radius:28px;font-size:18px;text-decoration:none">View offer</a></p>
<img src="https://t.northwind.example/open.gif?u=9" width="1" height="1" alt="">
</td></tr></table>
</td></tr></table>
</body></html>
"""

INLINE = """From: 设计组 <design@corp.example>
To: Alex <{me}>
Subject: 新版图标定稿（内嵌预览）
Date: {date}
Message-ID: <inline1@corp.example>
MIME-Version: 1.0
Content-Type: multipart/related; boundary="rel"

--rel
Content-Type: text/html; charset=utf-8

<p>Hi，新版图标已经定稿，预览如下：</p>
<p><img src="cid:preview@corp.example" width="96" height="96" alt="预览"></p>
<p>有意见周五前告诉我。</p>
--rel
Content-Type: image/png
Content-ID: <preview@corp.example>
Content-Transfer-Encoding: base64

iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==
--rel--
"""


def main():
    deliver("INBOX", text_mail("张三 <zhang@corp.example>", "周会纪要：Q4 规划与上线节奏",
        "大家好，\n\n明天下午三点在 5 楼会议室开周会。议程如下：\n\n1. Q4 目标回顾与调整\n2. 客户端轻量化方案评审（附件里有对比数据）\n3. 上线节奏：灰度两周，再全量\n\n请提前看一下附件，有问题会上讨论。\n\n张三",
        0.05, cc="李四 <lisi@corp.example>, 王五 <wangwu@corp.example>", msgid="weekly@corp.example"), 0.05)
    deliver("INBOX", with_attachment("云服务账单 <billing@cloud.example>", "你的收据 · 50 GB 存储方案",
        "订单号 MT4K2LQ9\n每月 ¥6.00，已从默认付款方式扣款。\n\n如有疑问请联系支持。", "收据-MT4K2LQ9.pdf", 0.2), 0.2)
    deliver("INBOX", text_mail("李四 <lisi@corp.example>", "Re: 周末去爬山吗？",
        "我周六有空，天气预报说是晴天，七点在地铁站见？我带水和零食。\n\n> 这周末要不要去爬山？\n> 想去北边那条新开的步道。",
        0.3, msgid="hike2@corp.example", reply_to="hike1@corp.example"), 0.3, "F")
    deliver("INBOX", NEWSLETTER.format(me=ME, date=date(0.4)), 0.4)
    deliver("INBOX", PROMO.format(me=ME, date=date(0.2)), 0.2)
    # Sent earlier from another of the user's addresses: LiteMail offers it as a sender.
    deliver("Sent", text_mail(f"Alex <alex@studio.example>", "Proposal for the spring shoot",
        "Attached is the proposal we discussed.", 4, to="Emily Chen <emily@studio.example>"), 4, "S")
    # Older than a 30-day window: only server search finds these.
    deliver("INBOX", text_mail("Paddle Club <hello@paddle.example>", "Your kayak rental receipt",
        "Thanks for renting the blue kayak. Total CA$48.00, paid by card.", 75), 75, "S")
    deliver("INBOX", text_mail("Emily Chen <emily@studio.example>", "Kayak trip photos",
        "Here are the photos from the lake trip. The kayak shots came out great!", 140), 140, "S")
    deliver("Archive", text_mail("Northwind Cars <hello@mailer.northwind.example>", "Your 2025 service invoice",
        "Invoice #8812 for the annual service. Kayak rack installed.", 260), 260, "S")
    # Apple Notes stores notes in a "Notes" mailbox; LiteMail must not show it.
    deliver("Notes", text_mail(f"Alex <{ME}>", "Shopping list", "milk, eggs", 3), 3)
    deliver("INBOX", INLINE.format(me=ME, date=date(1.2)), 1.2, "S")
    deliver("INBOX", text_mail("代码仓库 <noreply@code.example>", "[litemail] PR #42：同步窗口支持自定义天数",
        "王五 合并了 3 个提交到 main。\n\n变更：sync_days 为 0 时保留全部邮件；设置里新增自定义天数。\n\nhttps://code.example/litemail/pull/42", 1.4), 1.4)
    deliver("INBOX", with_attachment("航空公司 <trip@airline.example>", "行程确认：旧金山 → 东京",
        "您的电子客票已出票。\n航班 10月28日 13:05 起飞，请提前 3 小时到达机场。", "电子客票.pdf", 1.6), 1.6, "S")
    deliver("INBOX", with_attachment("王五 <wangwu@corp.example>", "合同终稿",
        "附件是终稿，请在周五前签署并回传。有任何修改请直接在文档里批注。", "合同终稿-v3.pdf", 3), 3, "SF")
    deliver("INBOX", text_mail("物业 <service@home.example>", "十月水电费账单", "本月水电共计 ¥186.40，已从账户自动扣款。", 6), 6, "S")
    deliver("INBOX", text_mail("Emily Chen <emily@studio.example>", "Coffee next week?",
        "Hi Alex,\n\nAre you free for coffee next Tuesday? I'd love to hear how the mail app is going.\n\nEmily", 9), 9, "S")
    deliver("INBOX", text_mail("读书会 <club@books.example>", "十一月书单投票", "本月候选：《人类简史》《置身事内》《三体》。回复数字投票。", 14), 14, "S")
    deliver("INBOX", text_mail("银行通知 <notice@bank.example>", "信用卡账单已出", "本期账单 ¥2,318.50，到期还款日 11月5日。", 22), 22, "S")
    deliver("INBOX", text_mail("老同学 <old@friends.example>", "聚会照片", "上次聚会的照片整理好了，回头发你网盘链接。", 40), 40, "S")
    deliver("INBOX", text_mail("房东 <landlord@home.example>", "续租确认", "明年的租约已经准备好了，有空来签一下。", 55), 55, "S")

    deliver("Sent", text_mail(f"Alex <{ME}>", "这周末要不要去爬山？", "想去北边那条新开的步道。",
        0.5, to="李四 <lisi@corp.example>", msgid="hike1@corp.example"), 0.5, "S")
    deliver("Sent", text_mail(f"Alex <{ME}>", "Re: Coffee next week?", "Tuesday works! 10am at the usual place?",
        8, to="Emily Chen <emily@studio.example>"), 8, "S")
    deliver("项目.2026" if False else "&mHl27g-.2026", text_mail("张三 <zhang@corp.example>", "2026 路线图草案",
        "附上 2026 年路线图草案，主要是性能和多账户。", 2), 2, "S")
    print(f"seeded demo mailbox for {USER} in {ROOT}")


if __name__ == "__main__":
    main()
