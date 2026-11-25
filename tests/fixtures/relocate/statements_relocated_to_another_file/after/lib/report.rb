class Report
  def render(rows)
    Csv.new(rows, separator: "\n").to_s
  end
end
