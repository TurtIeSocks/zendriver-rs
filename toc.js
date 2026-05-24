// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded "><a href="introduction.html"><strong aria-hidden="true">1.</strong> Introduction</a></li><li class="chapter-item expanded "><a href="install.html"><strong aria-hidden="true">2.</strong> Install</a></li><li class="chapter-item expanded "><a href="quickstart.html"><strong aria-hidden="true">3.</strong> Quickstart</a></li><li class="chapter-item expanded "><a href="stealth.html"><strong aria-hidden="true">4.</strong> Stealth</a></li><li class="chapter-item expanded "><a href="multi-tab.html"><strong aria-hidden="true">5.</strong> Multi-tab</a></li><li class="chapter-item expanded "><a href="frames.html"><strong aria-hidden="true">6.</strong> Frames</a></li><li class="chapter-item expanded "><a href="input.html"><strong aria-hidden="true">7.</strong> Input</a></li><li class="chapter-item expanded "><a href="interception.html"><strong aria-hidden="true">8.</strong> Interception</a></li><li class="chapter-item expanded "><a href="expect.html"><strong aria-hidden="true">9.</strong> Expect()</a></li><li class="chapter-item expanded "><a href="cloudflare.html"><strong aria-hidden="true">10.</strong> Cloudflare</a></li><li class="chapter-item expanded "><a href="fetcher.html"><strong aria-hidden="true">11.</strong> Fetcher</a></li><li class="chapter-item expanded "><a href="migration-playwright.html"><strong aria-hidden="true">12.</strong> Migration from Playwright</a></li><li class="chapter-item expanded "><a href="migration-zendriver-python.html"><strong aria-hidden="true">13.</strong> Migration from zendriver (Python)</a></li><li class="chapter-item expanded "><a href="migration-nodriver-python.html"><strong aria-hidden="true">14.</strong> Migration from nodriver (Python)</a></li><li class="chapter-item expanded "><a href="architecture.html"><strong aria-hidden="true">15.</strong> Architecture</a></li><li class="chapter-item expanded "><a href="faq.html"><strong aria-hidden="true">16.</strong> FAQ</a></li><li class="chapter-item expanded "><a href="error-reference.html"><strong aria-hidden="true">17.</strong> Error Reference</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0].split("?")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
