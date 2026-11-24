class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    t = Proc.new.call + a * 2
    puts t.to_s + 'zz' + a.to_s
  end
end
