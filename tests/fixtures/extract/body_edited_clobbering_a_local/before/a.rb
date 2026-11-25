class Report
  def render(title, rows)
    total = 0
    puts title
    total = rows.sum { |r| r[:amount] } * 2
    puts "-" * title.size
    puts total
  end
end
