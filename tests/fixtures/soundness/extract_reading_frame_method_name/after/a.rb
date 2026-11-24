class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    n = __method__
    puts n.to_s + a.to_s + 'x'
  end
end
