class Report
  def render(rows)
    Csv.new(rows).to_s
  end
end
