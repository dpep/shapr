class Report
  def summary
    head_line = title
    body_text = text
    foot_note = note
    [head_line, body_text, foot_note].join
  end

  def details
    rowCount = rows.size
    userName = owner
    render(rowCount, userName:)
  end
end
