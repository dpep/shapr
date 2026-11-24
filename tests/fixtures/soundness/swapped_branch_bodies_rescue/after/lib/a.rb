def f
  begin
    b.go
  rescue
    a.go
  end
end
