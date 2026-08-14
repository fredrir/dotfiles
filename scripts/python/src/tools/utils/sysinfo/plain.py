from tools.core.console import out
from tools.utils.sysinfo.branding import resolve_brand
from tools.utils.sysinfo.health import health_summary
from tools.utils.sysinfo.models import HealthIssue, RenderOptions, SystemView


def component_identity(component):
    brand = resolve_brand(component.kind, component.vendor, component.model, *component.identifiers)
    if brand.key == component.kind:
        return component.model
    return f"{brand.name} {component.model}".strip()


def render_health(issues):
    for issue in issues:
        out(f"{issue.severity.title()}: {issue.title}")
        if issue.detail:
            out(f"  {issue.detail}")
        if issue.action:
            out(f"  Action: {issue.action}")


def render_compact(view, issues):
    out("System: " + "  ".join(view.summary))
    for component in view.components:
        if component.compact:
            out(f"{component.label}: {component_identity(component)}")
    summary = health_summary(issues)
    if summary:
        out(f"Health: {summary}")


def render_full(view, issues):
    out("System")
    out(f"  Platform: {view.platform.label}")
    for item in view.system_facts:
        out(f"  {item.label}: {item.value}")
    out("Hardware")
    for component in view.components:
        out(f"  {component.label}: {component_identity(component)}")
        for item in component.facts:
            out(f"    {item.label}: {item.value}")
    if view.software:
        out("Software")
        for badge in view.software:
            out(f"  {badge.kind.title()}: {badge.label}")
    summary = health_summary(issues)
    if summary:
        out(f"Health: {summary}")


def render_plain(view: SystemView, issues: tuple[HealthIssue, ...], options: RenderOptions):
    if options.full:
        render_full(view, issues)
    else:
        render_compact(view, issues)
    if options.health and issues:
        out()
        render_health(issues)
