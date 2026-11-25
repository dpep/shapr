class A
  def report(name, total)
    header = compute_header(name)
    text = <<~EOS
        Report for #{name}
      #{header} total line
        Total is #{total}
    EOS
    deliver(text, header)
  end

  def other
    run_other_thing(1)
  end
end
