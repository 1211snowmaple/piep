// 実行前に確認した URL と、いま表示している文書が一致するときだけ読む。
(function (expectedUrl, source) {
    const url = document.URL;
    if (url !== expectedUrl) return { url, rows: [], truncated: false };

    const limit = 2000;
    const rows = new Map();
    const nameText = (element) => (element?.textContent || '').replace(/[\s\x00-\x1f\x7f-\x9f]+/g, ' ').trim();
    const result = (truncated = false) => ({ url, rows: [...rows.values()], truncated });

    if (source === 'pixiv') {
        for (const link of document.querySelectorAll('a[href*="/users/"]')) {
            const href = link.href;
            const name = nameText(link);
            if (!rows.has(href) && rows.size === limit) return result(true);
            if (name.length > (rows.get(href)?.name.length ?? -1)) rows.set(href, { href, name });
        }
        return result();
    }

    const supporting = new URL(url).pathname.split('/').includes('supporting');
    const listNames = supporting
        ? ['支援中', '支援中のクリエイター', 'Supporting', 'Supported Creators']
        : ['フォロー中', 'フォロー中のクリエイター', 'Following', 'Followed Creators'];
    const headingSelector = 'h1,h2,h3,h4,h5,h6,[role="heading"]';
    const listSelector = 'ul,ol,[role="list"]';
    const cardSelector = 'li,[role="listitem"]';
    const lists = new Set();
    const addList = (panel) => {
        if (!panel?.closest('main,[role="main"]')) return;
        if (panel.matches(listSelector)) lists.add(panel);
        for (const child of panel.children) {
            if (child.matches(listSelector)) lists.add(child);
        }
    };
    // 一覧と結び付いたタブ、または一覧名を見出しに持つ区画だけを使う。
    const current = new URL(url);
    for (const tab of document.querySelectorAll('a[aria-current="page"][aria-controls],a[role="tab"][aria-selected="true"][aria-controls]')) {
        const target = URL.parse(tab.href, url);
        if (!target) continue;
        if (target.origin === current.origin && target.pathname === current.pathname) {
            addList(document.getElementById(tab.getAttribute('aria-controls')));
        }
    }
    for (const section of document.querySelectorAll('main section,[role="main"] section')) {
        const heading = section.firstElementChild;
        if (heading?.matches(headingSelector) && listNames.includes(nameText(heading))) addList(section);
    }
    if (lists.size !== 1) return result();

    const creatorId = (href) => {
        const target = URL.parse(href, url);
        if (!target) return null;
        if (target.protocol !== 'https:') return null;
        const path = target.pathname.replace(/^\/(?:(?:ja|en|ko|zh-cn|zh-tw)\/)?/, '/');
        const id = ['www.fanbox.cc', 'fanbox.cc'].includes(target.hostname)
            ? path.match(/^\/@([a-z0-9_-]{1,64})\/?$/i)?.[1].toLowerCase()
            : path === '/' ? target.hostname.match(/^([a-z0-9_-]{1,64})\.fanbox\.cc$/)?.[1] : null;
        return id && !['www', 'api', 'downloads', 'help', 'official'].includes(id) ? id : null;
    };

    let candidates = 0;
    for (const card of [...lists][0].children) {
        if (!card.matches(cardSelector)) continue;
        const belongsToCard = (element) => element.closest(cardSelector) === card && !element.closest('p,blockquote');
        const names = [...card.querySelectorAll(`${headingSelector},[itemprop="name"]`)].filter(belongsToCard);
        const nameElement = names.length === 1 ? names[0] : null;
        const name = [...nameText(nameElement)].slice(0, 200).join('');
        const links = [];
        for (const link of card.querySelectorAll('a[href]')) {
            if (!belongsToCard(link)) continue;
            const isName = nameElement && (link.contains(nameElement) || nameElement.contains(link));
            const isIcon = !nameText(link) && link.querySelector('img,svg');
            if (!isName && !isIcon) continue;
            // href は文書の URL を基準にする。文書が指定した base 要素は使わない。
            const id = creatorId(link.getAttribute('href'));
            if (id) links.push(id);
        }
        if (new Set(links).size !== 1) continue;
        const [id] = links;
        if (candidates === limit) return result(true);
        if (name.length > (rows.get(id)?.name.length ?? -1)) {
            rows.set(id, { href: `https://www.fanbox.cc/@${id}`, name });
        }
        candidates += links.length;
        if (candidates > limit) return result(true);
    }
    return result();
})
