// 実行前に確認した URL と、いま表示している文書が一致するときだけ読む。
(function (expectedUrl, source) {
    const url = document.URL;
    if (url !== expectedUrl) return { url, rows: [], truncated: false };

    const candidateLimit = 2000;
    const linkScanLimit = 10000;
    // Rust が受け取れる JSON の上限。候補を加える前に UTF-8 のバイト数を確かめる。
    const responseLimit = 2 * 1024 * 1024;
    const encoder = new TextEncoder();
    const jsonBytes = (value) => encoder.encode(JSON.stringify(value)).byteLength;
    const rows = new Map();
    const result = (truncated = false) => ({ url, rows: [...rows.values()], truncated });
    let responseBytes = jsonBytes(result());
    const addRow = (id, href, name) => {
        const previous = rows.get(id);
        if (previous && name.length <= previous.name.length) return true;
        if (!previous && rows.size === candidateLimit) return false;

        const row = { href, name };
        const previousBytes = previous ? jsonBytes(previous) : 0;
        const commaBytes = !previous && rows.size > 0 ? 1 : 0;
        const nextBytes = responseBytes - previousBytes + jsonBytes(row) + commaBytes;
        if (nextBytes > responseLimit) return false;
        responseBytes = nextBytes;
        rows.set(id, row);
        return true;
    };

    const nameText = (element) => (element?.textContent || '').replace(/[\s\x00-\x1f\x7f-\x9f]+/g, ' ').trim();
    const candidateName = (element) => {
        let name = '';
        let length = 0;
        for (const char of nameText(element)) {
            name += char;
            if (++length === 200) break;
        }
        return name.toWellFormed();
    };

    const pixiv = source === 'pixiv';
    const current = new URL(url);
    const listPath = (target) => target.pathname.replace(/^\/(?:ja|en|ko|zh-cn|zh-tw)(?=\/)/, '').replace(/\/$/, '');
    const supporting = listPath(current) === '/creators/supporting';
    const listNames = pixiv
        ? ['フォロー', 'フォロー中', 'Following']
        : supporting
            ? ['支援中', '支援中のクリエイター', 'Supporting', 'Supported Creators']
            : ['フォロー中', 'フォロー中のクリエイター', 'Following', 'Followed Creators'];
    const headingSelector = 'h1,h2,h3,h4,h5,h6,[role="heading"]';
    const listSelector = 'ul,ol,[role="list"]';
    const cardSelector = 'li,[role="listitem"]';
    const nameSelector = headingSelector + ',[itemprop="name"]';
    const isVisible = (element) => {
        for (let node = element; node; node = node.parentElement) {
            if (node.matches('[hidden],[aria-hidden="true"]')) return false;
            const style = document.defaultView.getComputedStyle(node);
            if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse') return false;
        }
        return true;
    };

    const lists = new Set();
    const addList = (panel) => {
        if (!panel?.closest('main,[role="main"]')) return;
        if (panel.matches(listSelector) && isVisible(panel)) lists.add(panel);
        for (const child of panel.children) {
            if (child.matches(listSelector) && isVisible(child)) lists.add(child);
        }
    };

    // 文書の URL を基準にリンクを解決する。文書が指定した base 要素は使わない。
    const linkUrl = (link) => URL.parse(link.getAttribute('href'), current);
    // 選択中のタブの区画を優先し、結び付く一覧がなければ見出しで探す。
    for (const tab of document.querySelectorAll('a[href][aria-current="page"][aria-controls],a[href][role="tab"][aria-selected="true"][aria-controls]')) {
        const target = linkUrl(tab);
        if (target?.origin === current.origin && listPath(target) === listPath(current) && isVisible(tab)) {
            addList(document.getElementById(tab.getAttribute('aria-controls')));
        }
    }
    if (!lists.size) {
        for (const section of document.querySelectorAll('main section,[role="main"] section')) {
            const heading = section.firstElementChild;
            if (heading?.matches(headingSelector) && listNames.includes(nameText(heading))) addList(section);
        }
    }
    if (lists.size !== 1) return result();
    const [list] = lists;

    const owner = pixiv ? listPath(current).match(/^\/users\/([0-9]{1,20})\/following$/)?.[1] : null;
    const userId = (target) => {
        if (target.hostname !== 'pixiv.net' && !target.hostname.endsWith('.pixiv.net')) return null;
        return listPath(target).match(/^\/users\/([0-9]{1,20})$/)?.[1] ?? null;
    };
    const creatorId = (target) => {
        const path = listPath(target);
        const id = ['www.fanbox.cc', 'fanbox.cc'].includes(target.hostname)
            ? path.match(/^\/@([a-z0-9_-]{1,64})$/i)?.[1].toLowerCase()
            : path === '' ? target.hostname.match(/^([a-z0-9_-]{1,64})\.fanbox\.cc$/)?.[1] : null;
        return id && !['www', 'api', 'downloads', 'help', 'official'].includes(id) ? id : null;
    };

    let scannedLinks = 0;
    let acceptedCards = 0;
    for (const card of list.children) {
        if (!card.matches(cardSelector)) continue;
        const belongsToCard = (element) => element.closest(cardSelector) === card
            && element.closest(listSelector) === list && !element.closest('p,blockquote');
        let nameElement = null;
        if (!pixiv) {
            const names = [...card.querySelectorAll(nameSelector)].filter((element) => {
                if (!belongsToCard(element)) return false;
                const enclosingName = element.parentElement.closest(nameSelector);
                return !enclosingName || !belongsToCard(enclosingName);
            });
            if (names.length === 1) nameElement = names[0];
        }

        let creator = null;
        // カード内のリンクを先に配列へ集めず、走査の上限でその場で止める。
        const walker = document.createTreeWalker(card, NodeFilter.SHOW_ELEMENT);
        for (let link = walker.nextNode(); link; link = walker.nextNode()) {
            if (!link.matches('a[href]')) continue;
            if (scannedLinks === linkScanLimit) return result(true);
            scannedLinks += 1;
            if (!belongsToCard(link)) continue;

            if (!pixiv) {
                const isName = nameElement && (link.contains(nameElement) || nameElement.contains(link));
                const isIcon = !nameText(link) && link.querySelector('img,svg');
                if (!isName && !isIcon) continue;
            }
            const target = linkUrl(link);
            if (target?.protocol !== 'https:') continue;
            if (pixiv) {
                const id = userId(target);
                if (!id || id === owner) continue;
                const name = candidateName(link);
                if (name && !addRow(id, 'https://www.pixiv.net/users/' + id, name)) return result(true);
            } else {
                const id = creatorId(target);
                if (!id) continue;
                if (creator && creator !== id) {
                    creator = null;
                    break;
                }
                creator = id;
            }
        }
        if (creator) {
            if (acceptedCards === candidateLimit) return result(true);
            if (!addRow(creator, 'https://www.fanbox.cc/@' + creator, candidateName(nameElement))) return result(true);
            acceptedCards += 1;
        }
    }
    return result();
})
