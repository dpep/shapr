class Report
  def render(rows)
    header = rows.first.keys.join(",")
    body = rows.map { |row| row.values.join(",") }
    total = rows.sum { |row| row[:amount] }
    lines = [header, *body, "total,#{total}"]
    lines.join("\n")
  end
end
