class Report
  def render(rows)
    header = rows.first.keys.join(", ")
    banner = "Total due: #{rows.size}"
    puts [header, banner].join(" | ")
  end
end
