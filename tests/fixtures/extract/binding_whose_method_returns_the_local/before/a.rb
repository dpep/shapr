class Report
  def render(title, rows)
    puts title
    total = rows.map(&:amount).sum * 2 + rows.size
    puts total
  end
end
